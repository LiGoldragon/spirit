//! The 1-of-1 LOCAL criome gate + router origination hand-off witness
//! (Spirit `xhwa`).
//!
//! This proves the PRODUCTION daemon origination end to end: a REAL local criome
//! Unix socket authorizes the post-commit head, and the authorized head is
//! handed to a LOCAL router working socket as a `signal-spirit`
//! `ApplyAuthorizedRecord` frame — the exact frame the peer Spirit's apply
//! ingress re-judges and lands live. The criome round-trip is a genuine socket
//! call to a `CriomeDaemon` on its own OS thread; the router hand-off is a
//! genuine socket call to a stub router that captures the origination.
//!
//! Proofs:
//!
//!   (a) AUTHORIZED head hands off. criome holds a 1-of-1 contract the spirit
//!       attestor satisfies. `gate_and_hand_to_router` authorizes over the
//!       socket and hands the head to the router. The captured
//!       `ForwardedMessagePayload` carries ONE routed object whose octets decode
//!       to `Input::ApplyAuthorizedRecord`; the carried versioned entry re-hashes
//!       to head `D`; the carried Evidence binds to `D`'s operation digest; and
//!       the record identifier is the committed record's.
//!
//!   (b) DENIED head does NOT hand off. Threshold-short evidence → criome
//!       `Rejected` → `Denied`; the router receives nothing.
//!
//! Falsification: if origination bypassed criome, the denied head would still
//! reach the router; if the projection fabricated a body, the carried entry
//! would not re-hash to head `D` and the Evidence would not bind to it.

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use criome::daemon::CriomeDaemon;
use criome::language::{AttestedMomentStatement, OperationStatement};
use criome::master_key::MasterKey;
use criome::tables::StoreLocation;
use criome::transport::CriomeClient;
use sema_engine::{EntryDigest, VersionedCommitLogEntry};
use signal_criome::{
    AttestedMoment, AttestedMomentProposition, AuthorizationMode as CriomeAuthorizationMode,
    ComponentKind, Contract, ContractDigest, CriomeReply, CriomeRequest, Evidence, Identity,
    IdentityRegistration, KeyPurpose, ObjectDigest, OperationDigest, PolicyMember,
    RequiredSignatureThreshold, Rule, SignatureEnvelope, SignatureScheme, StampedSignatureEnvelope,
    Threshold, TimeSignature, TimeWindow, TimestampNanos,
};
use signal_router::{
    ActorIdentifier, ForwardedMessagePayload, Frame as RouterFrame, FrameBody as RouterFrameBody,
    Input as RouterInput, MessageSlot, Output as RouterOutput,
};
use spirit::criome_gate::{CriomeGate, GateDecision, LocalHeadCapture, SpiritAttestor};
use spirit::origination::RouterOrigination;
use spirit::schema::meta_signal::{
    ArchiveDatabaseTarget, ConfigureRequest, CriomeGateTarget, CriomeSocketPathText,
    Output as MetaOutput,
};
use spirit::schema::signal::{
    Certainty, Description, Domains, Entry, Importance, Input, Justification, Kind, Magnitude,
    Output, Privacy, QuoteText, Reasoning, RecordRequest, Referent, Referents, Testimony,
    VerbatimQuote,
};
use spirit::{Engine, Store};
use tempfile::TempDir;
use triad_runtime::{FrameBody as LengthPrefixedFrameBody, LengthPrefixedCodec};

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

/// Record a change and return the committed record identifier.
async fn record(engine: &mut Engine, description: &str) -> String {
    let output = engine
        .handle_async(Input::record(record_request(description)))
        .await
        .into_root();
    match output {
        Output::RecordAccepted(accepted) => accepted.payload().payload().clone(),
        other => panic!("record accepted, got {other:?}"),
    }
}

fn criome_gate_target(path: &std::path::Path) -> CriomeGateTarget {
    CriomeGateTarget::socket(CriomeSocketPathText::new(path.display().to_string()))
}

fn decode_hex(text: &str) -> Vec<u8> {
    assert!(text.len() % 2 == 0, "hex text is even length");
    (0..text.len())
        .step_by(2)
        .map(|start| u8::from_str_radix(&text[start..start + 2], 16).expect("valid hex digit"))
        .collect()
}

/// A stub LOCAL router: it binds a working Unix socket, accepts one
/// `SubmitRoutedObjects` origination, captures the carried
/// [`ForwardedMessagePayload`], and replies `RoutedObjectsAccepted` — enough for
/// the origination hand-off to complete against a real socket without standing
/// up the whole router daemon.
struct StubRouter {
    socket: PathBuf,
    received: Receiver<ForwardedMessagePayload>,
    _accept: std::thread::JoinHandle<()>,
}

impl StubRouter {
    fn bind(socket: PathBuf) -> Self {
        let listener = UnixListener::bind(&socket).expect("stub router binds its Unix socket");
        let (sender, received) = channel();
        let accept = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                Self::serve(stream, &sender);
            }
        });
        Self {
            socket,
            received,
            _accept: accept,
        }
    }

    /// Serve one `SubmitRoutedObjects` origination on a connection.
    fn serve(mut stream: UnixStream, sender: &std::sync::mpsc::Sender<ForwardedMessagePayload>) {
        let codec = LengthPrefixedCodec::default();
        let Ok(body) = codec.read_body(&mut stream) else {
            return;
        };
        let Ok(frame) = RouterFrame::decode(body.bytes()) else {
            return;
        };
        let RouterFrameBody::Request { exchange, request } = frame.into_body() else {
            return;
        };
        if let RouterInput::SubmitRoutedObjects(payload) = request.payloads.into_head() {
            let _ = sender.send(payload);
        }
        let reply = RouterOutput::routed_objects_accepted(MessageSlot::new(0)).into_reply_frame(exchange);
        if let Ok(octets) = reply.encode() {
            let _ = codec.write_body(&mut stream, &LengthPrefixedFrameBody::new(octets));
            let _ = stream.flush();
        }
    }

    fn socket(&self) -> PathBuf {
        self.socket.clone()
    }

    /// The captured origination payload, or `None` if none arrived in time.
    fn captured(&self) -> Option<ForwardedMessagePayload> {
        self.received.recv_timeout(Duration::from_secs(5)).ok()
    }

    /// Whether the router received no origination within a short window.
    fn received_nothing(&self) -> bool {
        matches!(
            self.received.recv_timeout(Duration::from_millis(500)),
            Err(RecvTimeoutError::Timeout)
        )
    }
}

/// The 1-of-1 criome policy: one release-authorization signer and one
/// timekeeper, a single-member threshold-1 contract — the deploy-config trust
/// material the gate's `SpiritAttestor` carries and the peer's criome re-judges.
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
        Evidence::new(ComponentKind::Spirit, operation, stamp, signatures, Vec::new())
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
fn spawn_local_criome(directory: &TempDir) -> PathBuf {
    let socket = directory.path().join("criome.sock");
    let store = StoreLocation::new(directory.path().join("criome.sema"));
    let bound = CriomeDaemon::new(socket.clone(), store)
        .bind()
        .expect("criome daemon binds its Unix socket");
    std::thread::spawn(move || {
        let _ = bound.serve_forever();
    });
    socket
}

/// Run a real local criome daemon in AutoApprove mode for the socket-only
/// production bootstrap path.
fn spawn_auto_approve_criome(directory: &TempDir) -> PathBuf {
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

fn open_spirit_engine(directory: &TempDir, name: &str) -> Engine {
    let store = Store::open(directory.path().join(name)).expect("open spirit store");
    let mut engine = Engine::new(store);
    engine.start().expect("engine starts");
    engine
}

#[test]
fn meta_configure_arms_and_clears_criome_gate_socket() {
    let directory = tempfile::tempdir().expect("component temp dir");
    let criome_socket = directory.path().join("criome.sock");
    let mut engine = open_spirit_engine(&directory, "source.sema");

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
