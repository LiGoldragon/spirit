//! The 1-of-1 LOCAL criome gate witness (Spirit `xhwa`, report 703-6 Item 1).
//!
//! This proves the PRODUCTION daemon gate end to end through a REAL local
//! criome Unix socket — the gate's `CriomeClient::send` does a genuine socket
//! round-trip to a criome daemon (`BoundCriomeDaemon::serve_forever` on its own
//! OS thread), not an in-process `ActorRef` ask. The spirit side is the real
//! `Engine::gate_and_ship_head` the daemon's `handle_working_input` calls, armed
//! against a live in-process mirror so the FAN-OUT is observable.
//!
//! Three proofs, one binary:
//!
//!   (a) AUTHORIZED D — criome holds a 1-of-1 contract the spirit attestor
//!       satisfies. `gate_and_ship_head` calls criome over the socket, gets
//!       `Authorized`, emits the PROJECTED reference (`{ Spirit, D, Head }`,
//!       the digest matching head D by construction), AND the mirror receives
//!       the shipped suffix — the outbox drains, durability is `ServerCommitted`.
//!
//!   (b) DENIED D — the attestor's evidence is threshold-short, so criome
//!       returns a `Rejected` decision. `gate_and_ship_head` returns `Denied`
//!       and the head does NOT ship: the outbox stays queued, durability stays
//!       `QueuedForMirror`. The local commit stands; nothing fans out.
//!
//!   (c) UNCONFIGURED D — no local criome socket + attestor are configured.
//!       `gate_and_ship_head` returns `Unconfigured` and does NOT ship. Missing
//!       authorization is not a legacy pass-through.
//!
//! Falsification: if the gate shipped without consulting criome, the denied
//! case would drain the outbox; if the projection fabricated a reference, the
//! authorized reference's digest would not equal head D's digest.

use std::net::SocketAddr;

use criome::daemon::CriomeDaemon;
use criome::language::{AttestedMomentStatement, OperationStatement};
use criome::tables::StoreLocation;
use criome::transport::CriomeClient;
use mirror::{Engine as MirrorEngine, Service, ServiceLink};
use sema_engine::{Durability, EntryDigest};
use signal_criome::{
    AttestedMoment, AttestedMomentProposition, AuthorizationMode as CriomeAuthorizationMode,
    ComponentKind, Contract, ContractDigest, CriomeReply, CriomeRequest, Evidence, Identity,
    IdentityRegistration, KeyPurpose, ObjectDigest, OperationDigest, PolicyMember,
    RequiredSignatureThreshold, Rule, SignatureEnvelope, SignatureScheme, StampedSignatureEnvelope,
    Threshold, TimeSignature, TimeWindow, TimestampNanos,
};
use signal_spirit::AuthorizationMode;
use spirit::criome_gate::{CriomeGate, GateDecision, LocalHeadCapture, SpiritAttestor};
use spirit::schema::meta_signal::{
    ArchiveDatabaseTarget, ConfigureRequest, CriomeGateTarget, CriomeSocketPathText, MirrorAddress,
    MirrorAddressText, MirrorTarget, Output as MetaOutput,
};
use spirit::schema::sema::RecordFamily;
use spirit::schema::signal::{
    Certainty, Description, Domains, Entry, Importance, Input, Justification, Kind, Magnitude,
    Output, Privacy, QuoteText, Reasoning, RecordRequest, Referent, Referents, Testimony,
    VerbatimQuote,
};
use spirit::{Engine, Store};
use tempfile::TempDir;
use triad_runtime::kameo::actor::Spawn;

use criome::master_key::MasterKey;

const STORE_NAME: &str = RecordFamily::STORE_NAME;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn record_request(description: &str) -> RecordRequest {
    RecordRequest {
        entry: Entry {
            domains: Domains::from_strings(vec![String::from("Information/Documentation")]),
            kind: Kind::Decision,
            description: Description::new(description),
            certainty: Certainty::new(Magnitude::High),
            importance: Importance::new(Magnitude::Medium),
            privacy: Privacy::new(Magnitude::Zero),
            referents: Referents::new(vec![Referent::new("spirit")]),
        },
        justification: Justification {
            testimony: Testimony::new(vec![VerbatimQuote::new(QuoteText::new(description), None)]),
            reasoning: Reasoning::new(description),
        },
    }
}

async fn record(engine: &mut Engine, description: &str) {
    let output = engine
        .handle_async(Input::record(record_request(description)))
        .await
        .into_root();
    assert!(
        matches!(output, Output::RecordAccepted(_)),
        "record accepted, got {output:?}"
    );
}

fn mirror_target(address: SocketAddr) -> MirrorTarget {
    MirrorTarget::Address(MirrorAddress::new(MirrorAddressText::new(
        address.to_string(),
    )))
}

fn criome_gate_target(path: &std::path::Path) -> CriomeGateTarget {
    CriomeGateTarget::socket(CriomeSocketPathText::new(path.display().to_string()))
}

/// Stand up an in-process mirror daemon (real engine, real store, loopback TCP)
/// and register the spirit store on its meta surface — the fan-out target.
async fn running_mirror(directory: &TempDir) -> (ServiceLink, SocketAddr) {
    let store =
        mirror::Store::open(&directory.path().join("mirror.sema")).expect("mirror store opens");
    let service = Service::spawn(Service::new(
        MirrorEngine::new(store),
        "127.0.0.1:0".parse().expect("loopback address"),
    ));
    service.wait_for_startup().await;
    let link = ServiceLink::new(service);
    let address = link
        .tcp_bound_address()
        .await
        .expect("query bound address")
        .expect("the tailnet ingress is bound");
    let registered = link
        .meta(meta_signal_mirror::Input::RegisterStore(
            meta_signal_mirror::StoreRegistration {
                store: meta_signal_mirror::StoreName::new(STORE_NAME.to_owned()),
                addressing: meta_signal_mirror::ContentAddressing::Opaque,
            },
        ))
        .await
        .expect("meta register");
    assert!(matches!(
        registered,
        meta_signal_mirror::Output::StoreRegistered(_)
    ));
    (link, address)
}

/// The 1-of-1 criome policy: one release-authorization signer and one
/// timekeeper, a single-member threshold-1 contract. This is the deploy-config
/// trust material the gate's `SpiritAttestor` carries.
struct LocalCriomePolicy {
    signer_identity: Identity,
    signer_key: MasterKey,
    timekeeper_identity: Identity,
    timekeeper_key: MasterKey,
}

impl LocalCriomePolicy {
    fn new() -> Self {
        Self {
            signer_identity: Identity::developer("spirit-local-signer".to_owned()),
            signer_key: MasterKey::generate().expect("signer key generates"),
            timekeeper_identity: Identity::cluster("spirit-local-timekeeper".to_owned()),
            timekeeper_key: MasterKey::generate().expect("timekeeper key generates"),
        }
    }

    fn registration(identity: &Identity, key: &MasterKey) -> IdentityRegistration {
        IdentityRegistration::new(
            identity.clone(),
            key.public_key(),
            key.fingerprint(),
            KeyPurpose::ReleaseAuthorization,
            None,
        )
    }

    /// The threshold-1, single-member contract: one signature from the local
    /// signer satisfies it (criome's `k > n/2` admits n=1, k=1).
    fn contract() -> Contract {
        Contract::new(Rule::Threshold(Threshold::new(
            RequiredSignatureThreshold::new(1),
            vec![PolicyMember::KeyMember(Identity::developer(
                "spirit-local-signer".to_owned(),
            ))],
        )))
    }

    /// A timekeeper-signed attested moment over a valid (opens < closes) window.
    fn stamp(&self) -> AttestedMoment {
        let proposition = AttestedMomentProposition::new(
            TimeWindow {
                opens_at: TimestampNanos::new(10),
                closes_at: TimestampNanos::new(20),
            },
            RequiredSignatureThreshold::new(1),
            vec![self.timekeeper_identity.clone()],
        );
        let signature = TimeSignature {
            signer: self.timekeeper_identity.clone(),
            envelope: SignatureEnvelope {
                scheme: SignatureScheme::Bls12_381MinPk,
                public_key: self.timekeeper_key.public_key(),
                signature: self.timekeeper_key.sign(
                    AttestedMomentStatement::new(&proposition)
                        .to_signing_bytes()
                        .expect("moment statement signs")
                        .as_slice(),
                ),
            },
        };
        AttestedMoment::new(proposition, vec![signature])
    }

    /// Evidence over the head's operation digest, signed by `signer_count`
    /// distinct signers. `signer_count == 1` satisfies the threshold-1 contract;
    /// `signer_count == 0` is threshold-short and is REJECTED.
    fn evidence(&self, operation: OperationDigest, signer_count: usize) -> Evidence {
        let stamp = self.stamp();
        let signatures: Vec<StampedSignatureEnvelope> = if signer_count == 0 {
            Vec::new()
        } else {
            let statement = OperationStatement::new(&self.signer_identity, &operation, &stamp)
                .to_signing_bytes()
                .expect("operation statement signs");
            vec![StampedSignatureEnvelope {
                stamp: stamp.clone(),
                envelope: SignatureEnvelope {
                    scheme: SignatureScheme::Bls12_381MinPk,
                    public_key: self.signer_key.public_key(),
                    signature: self.signer_key.sign(&statement),
                },
            }]
        };
        Evidence::new(
            ComponentKind::Spirit,
            operation,
            stamp,
            signatures,
            Vec::new(),
        )
    }

    /// Seed the running criome daemon over the socket: register both identities
    /// and admit the contract. Returns the admitted contract digest.
    fn seed(&self, socket: &std::path::Path) -> ContractDigest {
        let client = CriomeClient::new(socket);
        for (identity, key) in [
            (&self.signer_identity, &self.signer_key),
            (&self.timekeeper_identity, &self.timekeeper_key),
        ] {
            let reply = client
                .send(CriomeRequest::RegisterIdentity(Self::registration(
                    identity, key,
                )))
                .expect("identity registration reaches criome over the socket");
            assert!(
                matches!(reply, CriomeReply::IdentityReceipt(_)),
                "identity registered, got {reply:?}"
            );
        }
        let reply = client
            .send(CriomeRequest::AdmitContract(Self::contract()))
            .expect("contract admission reaches criome over the socket");
        let CriomeReply::ContractAdmitted(admitted) = reply else {
            panic!("expected ContractAdmitted, got {reply:?}");
        };
        admitted.into_payload()
    }
}

/// Run a real criome daemon over a fresh Unix socket on its own OS thread,
/// serving connections forever. Returns the socket path (kept alive by the
/// owned temp dir the caller holds).
fn spawn_local_criome(directory: &TempDir) -> std::path::PathBuf {
    let socket = directory.path().join("criome.sock");
    let store = StoreLocation::new(directory.path().join("criome.sema"));
    let bound = CriomeDaemon::new(socket.clone(), store)
        .bind()
        .expect("criome daemon binds its Unix socket");
    std::thread::spawn(move || {
        // serve_forever ends when the listener errors (process teardown).
        let _ = bound.serve_forever();
    });
    // The bind() above created the socket file before returning, so the gate's
    // client will find it.
    socket
}

/// Run a real local criome daemon in AutoApprove mode for the socket-only
/// production bootstrap path. Approval still means criome returns a signed
/// `AuthorizationGrant`; the request shape is simple, not the answer.
fn spawn_auto_approve_criome(directory: &TempDir) -> std::path::PathBuf {
    let socket = directory.path().join("criome-auto.sock");
    let store = StoreLocation::new(directory.path().join("criome-auto.sema"));
    let bound = CriomeDaemon::new(socket.clone(), store)
        .with_authorization_mode(CriomeAuthorizationMode::AutoApprove)
        .bind()
        .expect("criome daemon binds its Unix socket");
    std::thread::spawn(move || {
        let _ = bound.serve_forever();
    });
    socket
}

/// Open a fresh spirit engine armed at the in-process mirror.
fn armed_spirit_engine(directory: &TempDir, name: &str, mirror_address: SocketAddr) -> Engine {
    let store = Store::open(directory.path().join(name)).expect("open spirit store");
    let mut engine = Engine::new(store);
    engine.start().expect("engine starts");
    let configured = engine.configure(ConfigureRequest::new(
        ArchiveDatabaseTarget::Default,
        Some(mirror_target(mirror_address)),
        None,
        None,
    ));
    assert!(
        matches!(configured, MetaOutput::Configured(_)),
        "configure accepted, got {configured:?}"
    );
    assert!(engine.mirror_shipping_armed(), "the shipper is armed");
    engine
}

#[test]
fn meta_configure_arms_and_clears_criome_gate_socket() {
    let directory = tempfile::tempdir().expect("component temp dir");
    let criome_socket = directory.path().join("criome.sock");
    let store = Store::open(directory.path().join("source.sema")).expect("open spirit store");
    let mut engine = Engine::new(store);
    engine.start().expect("engine starts");

    let configured = engine.configure(ConfigureRequest::new(
        ArchiveDatabaseTarget::Default,
        None,
        Some(criome_gate_target(&criome_socket)),
        None,
    ));
    assert!(
        matches!(configured, MetaOutput::Configured(_)),
        "configure accepted, got {configured:?}"
    );
    assert!(
        engine.criome_gate_armed(),
        "meta Configure arms criome gate"
    );

    let cleared = engine.configure(ConfigureRequest::new(
        ArchiveDatabaseTarget::Default,
        None,
        Some(CriomeGateTarget::Default),
        None,
    ));
    assert!(
        matches!(cleared, MetaOutput::Configured(_)),
        "clear configure accepted, got {cleared:?}"
    );
    assert!(
        !engine.criome_gate_armed(),
        "meta Configure(Default) clears criome gate"
    );
}

#[test]
fn socket_only_gate_observes_signed_auto_approved_authorization() {
    let runtime = runtime();
    let directory = tempfile::tempdir().expect("criome temp dir");
    let criome_socket = spawn_auto_approve_criome(&directory);
    let mut gate = CriomeGate::new();
    gate.configure_socket(&criome_socket);

    let capture = LocalHeadCapture::spirit_head(EntryDigest::new([42; 32]));
    let decision = runtime
        .block_on(gate.observe_authorization(&capture))
        .expect("socket-only gate observes criome authorization");
    let GateDecision::Observed(observed) = decision else {
        panic!("expected observed authorization, got {decision:?}");
    };
    assert!(observed.authorized());
    assert_eq!(observed.reference().component, ComponentKind::Spirit);
    assert_eq!(
        observed.reference().digest,
        ObjectDigest::from_bytes(capture.head_digest().bytes())
    );
}

#[test]
fn authorized_head_ships_and_emits_projected_reference_denied_head_does_not_ship() {
    let runtime = runtime();
    let mirror_a_directory = tempfile::tempdir().expect("mirror A temp dir");
    let mirror_b_directory = tempfile::tempdir().expect("mirror B temp dir");
    let criome_directory = tempfile::tempdir().expect("criome temp dir");
    let authorized_directory = tempfile::tempdir().expect("authorized component temp dir");
    let denied_directory = tempfile::tempdir().expect("denied component temp dir");

    // The real local criome daemon over a Unix socket, seeded with a 1-of-1
    // contract — spirit's gate will round-trip to it.
    let criome_socket = spawn_local_criome(&criome_directory);
    let policy = LocalCriomePolicy::new();
    let contract = policy.seed(&criome_socket);

    runtime.block_on(async {
        let (link_a, mirror_address) = running_mirror(&mirror_a_directory).await;

        // ============ PROOF (a): AUTHORIZED D ships + emits projection =========
        let mut engine = armed_spirit_engine(&authorized_directory, "source.sema", mirror_address);
        record(&mut engine, "the authorized head fans out").await;

        // Capture head D exactly as the gate does, so we can assert the emitted
        // reference's digest equals head D's digest (projection, not fabrication).
        let head_digest = engine
            .versioned_log_head()
            .expect("versioned head reads")
            .expect("a committed head exists");
        let expected_object = ObjectDigest::from_bytes(head_digest.bytes());

        // Arm the gate with an attestor whose evidence satisfies the contract.
        let operation = OperationDigest::from_bytes(head_digest.bytes());
        engine.arm_criome_gate(
            &criome_socket,
            SpiritAttestor::new(contract.clone(), policy.evidence(operation, 1)),
        );
        assert!(engine.criome_gate_armed(), "the criome gate is armed");

        // Before the gate runs, the local history is queued for the mirror.
        let handle = engine.store().engine_handle();
        assert_eq!(
            handle.store_durability().expect("durability reads"),
            Durability::QueuedForMirror
        );

        // THE GATE: capture D → ask criome over the socket → ship only on
        // Authorized. This is exactly what the daemon's handle_working_input
        // calls.
        let decision = engine
            .gate_and_ship_head()
            .await
            .expect("the gate completes without machinery fault")
            .expect("a head exists to authorize");
        let GateDecision::Authorized(reference) = decision else {
            panic!("expected Authorized over the socket, got {decision:?}");
        };
        // The emitted reference is the PROJECTION of head D — same digest, the
        // Spirit component, the Head kind.
        assert_eq!(reference.component, ComponentKind::Spirit);
        assert_eq!(reference.kind, signal_criome::AuthorizedObjectKind::Head);
        assert_eq!(
            reference.digest, expected_object,
            "the authorized reference is head D's digest, projected not fabricated"
        );

        // The authorized head FANNED OUT: the outbox drained, the shared engine
        // marks the shipped history server-committed.
        assert_eq!(
            handle.store_durability().expect("durability reads"),
            Durability::ServerCommitted,
            "an authorized head ships to the mirror"
        );
        assert!(
            handle.unshipped_outbox().expect("outbox reads").is_empty(),
            "the authorized ship covers the whole outbox"
        );
        drop(link_a);

        // ============ PROOF (b): DENIED D does NOT ship =======================
        let (link_b, mirror_address_b) = running_mirror(&mirror_b_directory).await;
        let mut denied = armed_spirit_engine(&denied_directory, "denied.sema", mirror_address_b);
        record(&mut denied, "the denied head must not fan out").await;

        let denied_head = denied
            .versioned_log_head()
            .expect("versioned head reads")
            .expect("a committed head exists");
        let denied_operation = OperationDigest::from_bytes(denied_head.bytes());
        // Threshold-short evidence (zero operation signatures) → criome rejects.
        denied.arm_criome_gate(
            &criome_socket,
            SpiritAttestor::new(contract.clone(), policy.evidence(denied_operation, 0)),
        );

        let denied_handle = denied.store().engine_handle();
        assert_eq!(
            denied_handle.store_durability().expect("durability reads"),
            Durability::QueuedForMirror
        );

        let decision = denied
            .gate_and_ship_head()
            .await
            .expect("the gate completes without machinery fault")
            .expect("a head exists to authorize");
        assert!(
            matches!(decision, GateDecision::Denied(_)),
            "a threshold-short head is denied over the socket, got {decision:?}"
        );

        // The denied head did NOT fan out: the outbox stays queued, the local
        // commit stands alone.
        assert_eq!(
            denied_handle.store_durability().expect("durability reads"),
            Durability::QueuedForMirror,
            "a denied head must not ship — the local commit stands, nothing fans out"
        );
        assert!(
            !denied_handle
                .unshipped_outbox()
                .expect("outbox reads")
                .is_empty(),
            "the denied write stays unshipped in the outbox"
        );
        drop(link_b);

        // ============ PROOF (c): OBSERVING D receives criome's answer and still ships ==========
        let mirror_observing_directory = tempfile::tempdir().expect("mirror observing temp dir");
        let observing_directory = tempfile::tempdir().expect("observing component temp dir");
        let (observing_link, observing_mirror_address) =
            running_mirror(&mirror_observing_directory).await;
        let mut observing = armed_spirit_engine(
            &observing_directory,
            "observing.sema",
            observing_mirror_address,
        );
        observing.set_authorization_mode(AuthorizationMode::Observing);
        record(
            &mut observing,
            "the observing head emits criome authorization and still fans out",
        )
        .await;

        let observing_head = observing
            .versioned_log_head()
            .expect("versioned head reads")
            .expect("a committed head exists");
        let observing_operation = OperationDigest::from_bytes(observing_head.bytes());
        observing.arm_criome_gate(
            &criome_socket,
            SpiritAttestor::new(contract.clone(), policy.evidence(observing_operation, 1)),
        );

        let observing_handle = observing.store().engine_handle();
        assert_eq!(
            observing_handle
                .store_durability()
                .expect("durability reads"),
            Durability::QueuedForMirror
        );

        let decision = observing
            .gate_and_ship_head()
            .await
            .expect("the observing gate completes without machinery fault")
            .expect("a head exists to authorize");
        let GateDecision::Observed(observed) = decision else {
            panic!("expected observing mode to receive criome's verdict without blocking fan-out, got {decision:?}");
        };
        let reference = observed.reference();
        assert!(observed.authorized());
        assert_eq!(reference.component, ComponentKind::Spirit);
        assert_eq!(reference.kind, signal_criome::AuthorizedObjectKind::Head);
        assert_eq!(
            reference.digest.clone(),
            ObjectDigest::from_bytes(observing_head.bytes())
        );
        assert_eq!(
            observing_handle
                .store_durability()
                .expect("durability reads"),
            Durability::ServerCommitted,
            "observing mode ships after seeing criome's non-blocking authorization"
        );
        assert!(
            observing_handle
                .unshipped_outbox()
                .expect("outbox reads")
                .is_empty(),
            "observing mode drains the outbox after emitting the request"
        );
        drop(observing_link);

        // ============ PROOF (d): UNCONFIGURED D does NOT ship ===============
        let mirror_c_directory = tempfile::tempdir().expect("mirror C temp dir");
        let unconfigured_directory = tempfile::tempdir().expect("unconfigured component temp dir");
        let (link_c, mirror_address_c) = running_mirror(&mirror_c_directory).await;
        let mut unconfigured = armed_spirit_engine(
            &unconfigured_directory,
            "unconfigured.sema",
            mirror_address_c,
        );
        record(&mut unconfigured, "the unconfigured gate must not fan out").await;

        let unconfigured_handle = unconfigured.store().engine_handle();
        assert_eq!(
            unconfigured_handle
                .store_durability()
                .expect("durability reads"),
            Durability::QueuedForMirror
        );

        let decision = unconfigured
            .gate_and_ship_head()
            .await
            .expect("the gate completes without machinery fault")
            .expect("a head exists to authorize");
        assert!(
            matches!(decision, GateDecision::Unconfigured),
            "an unconfigured local criome gate must hold the head back, got {decision:?}"
        );
        assert_eq!(
            unconfigured_handle
                .store_durability()
                .expect("durability reads"),
            Durability::QueuedForMirror,
            "an unconfigured gate must not ship"
        );
        assert!(
            !unconfigured_handle
                .unshipped_outbox()
                .expect("outbox reads")
                .is_empty(),
            "the unconfigured write stays unshipped in the outbox"
        );
        drop(link_c);
    });
}
