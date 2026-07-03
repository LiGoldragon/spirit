//! THE QUORUM ORIGINATION BOUNDARY WITNESS (primary-nbmq.10).
//!
//! Spirit's rewired origination no longer submits a caller-assembled Evidence to
//! a 1-of-1 gate for an immediate verdict (the `.3` de-risk join). It proposes the
//! committed head's operation digest to the LOCAL criome under an admitted 2-of-2
//! mirror quorum contract, criome gathers the peer's vote across the voice, and
//! Spirit hands the head + the quorum-ASSEMBLED Evidence to the LOCAL router ONLY
//! on the round's `Authorized` verdict.
//!
//! Two proofs, both driving the production seam `Engine::gate_and_hand_to_router`:
//!   (a) with both criomes up, the round gathers a REAL 2-of-2 BLS majority across
//!       the voice and Spirit ships the head carrying an Evidence with BOTH
//!       members' signatures — not a fabricated 1-of-1 Evidence;
//!   (b) with the peer criome unreachable, the round stays `Gathering`, the bounded
//!       ship budget expires, and NOTHING is handed to the router — the change is
//!       WITHHELD, never last-writer-wins.

use std::io::Write as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use criome::daemon::CriomeDaemon;
use criome::tables::StoreLocation;
use criome::transport::CriomeClient;
use criome::voice::{DirectDialQuorumVoice, PeerSocketRoute};
use sema_engine::VersionedCommitLogEntry;
use signal_criome::{
    AuditContext, BlsPublicKey, Contract, ContentPurpose, ContentReference, ContractDigest,
    CriomeReply, CriomeRequest, Evidence, Identity, IdentityRegistration, KeyPurpose, ObjectDigest,
    PolicyMember, PrincipalName, PublicKeyFingerprint, ReplayNonce, RequiredSignatureThreshold,
    Rule, SignRequest, Threshold,
};
use signal_router::{
    ActorIdentifier, Frame as RouterFrame, FrameBody as RouterFrameBody, ForwardedMessagePayload,
    Input as RouterInput, MessageSlot, Output as RouterOutput,
};
use spirit::criome_gate::SpiritAttestor;
use spirit::origination::{QuorumCompletionBudget, QuorumOriginationOutcome, RouterOrigination};
use spirit::schema::signal::{
    Certainty, Description, Domains, Entry, Importance, Input as SpiritInput, Justification, Kind,
    Magnitude, Output as SpiritOutput, Privacy, QuoteText, Reasoning, RecordRequest, Referent,
    Referents, Testimony, VerbatimQuote,
};
use spirit::{Engine, Store};
use tempfile::TempDir;
use triad_runtime::{FrameBody as LengthPrefixedFrameBody, LengthPrefixedCodec};

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn host(name: &str) -> Identity {
    Identity::host(name.to_string())
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

async fn record(engine: &mut Engine, description: &str) -> String {
    match engine
        .handle_async(SpiritInput::record(record_request(description)))
        .await
        .into_root()
    {
        SpiritOutput::RecordAccepted(accepted) => accepted.payload().payload().clone(),
        other => panic!("record accepted, got {other:?}"),
    }
}

fn open_spirit_engine(directory: &TempDir, name: &str) -> Engine {
    let store = Store::open(directory.path().join(name)).expect("open spirit store");
    let mut engine = Engine::new(store);
    engine.start().expect("spirit engine starts");
    engine
}

/// A stub LOCAL router: binds a working Unix socket, accepts `SubmitRoutedObjects`
/// originations, captures each carried [`ForwardedMessagePayload`], and replies
/// `RoutedObjectsAccepted` — enough for the origination hand-off to complete
/// against a real socket without standing up the whole router daemon.
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
        let reply =
            RouterOutput::routed_objects_accepted(MessageSlot::new(0)).into_reply_frame(exchange);
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
        self.received.recv_timeout(Duration::from_secs(10)).ok()
    }

    /// Whether the router received no origination within a short window.
    fn received_nothing(&self) -> bool {
        matches!(
            self.received.recv_timeout(Duration::from_millis(800)),
            Err(RecvTimeoutError::Timeout)
        )
    }
}

fn wait_for_socket(socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "criome socket never appeared: {socket:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn ask(socket: &Path, request: CriomeRequest) -> CriomeReply {
    CriomeClient::new(socket)
        .send(request)
        .unwrap_or_else(|error| panic!("criome round-trip on {socket:?}: {error}"))
}

/// Discover a node's master public key by asking it to sign a fixture as itself.
fn node_public_key(socket: &Path, identity: Identity) -> BlsPublicKey {
    let request = SignRequest::new(
        ContentReference {
            digest: ObjectDigest::from_bytes(b"quorum-key-probe"),
            purpose: ContentPurpose::SignedObject,
            schema_version: PrincipalName::new("quorum-probe"),
        },
        identity,
        AuditContext {
            purpose: ContentPurpose::SignedObject,
            audience: PrincipalName::new("quorum-probe-audience"),
            policy_version: PrincipalName::new("quorum-probe-policy"),
            nonce: ReplayNonce::new("quorum-probe-nonce"),
        },
        None,
    );
    match ask(socket, CriomeRequest::Sign(request)) {
        CriomeReply::SignReceipt(receipt) => receipt.attestation.envelope.public_key,
        other => panic!("expected SignReceipt, got {other:?}"),
    }
}

fn register_peer(socket: &Path, identity: Identity, public_key: BlsPublicKey) {
    let registration = IdentityRegistration::new(
        identity.clone(),
        public_key,
        PublicKeyFingerprint::new(format!("{identity:?}-fingerprint")),
        KeyPurpose::CriomeRoot,
        None,
    );
    match ask(socket, CriomeRequest::RegisterIdentity(registration)) {
        CriomeReply::IdentityReceipt(_) => {}
        other => panic!("expected IdentityReceipt, got {other:?}"),
    }
}

fn admit(socket: &Path, contract: Contract) -> ContractDigest {
    match ask(socket, CriomeRequest::AdmitContract(contract)) {
        CriomeReply::ContractAdmitted(admitted) => admitted.into_payload(),
        other => panic!("expected ContractAdmitted, got {other:?}"),
    }
}

fn mirror_contract(alpha: &Identity, beta: &Identity) -> Contract {
    Contract::new(Rule::threshold(Threshold::new(
        RequiredSignatureThreshold::new(2),
        vec![
            PolicyMember::key_member(alpha.clone()),
            PolicyMember::key_member(beta.clone()),
        ],
    )))
}

fn decode_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("valid hex byte"))
        .collect()
}

/// Extract the carried Evidence and record identifier from a captured origination.
fn carried_evidence(payload: &ForwardedMessagePayload) -> (String, Evidence) {
    assert_eq!(payload.routed_objects().len(), 1, "one routed object");
    let object = &payload.routed_objects()[0];
    assert_eq!(object.contract_name.payload(), "signal-spirit");
    assert_eq!(object.contract_operation.payload(), "ApplyAuthorizedRecord");
    let frame_octets: Vec<u8> = object
        .payload_octets()
        .iter()
        .map(|octet| u8::try_from(*octet).expect("octet fits u8"))
        .collect();
    let (_route, input) =
        SpiritInput::decode_signal_frame(&frame_octets).expect("octets decode as a spirit input");
    let SpiritInput::ApplyAuthorizedRecord(apply) = input else {
        panic!("expected ApplyAuthorizedRecord, got {input:?}");
    };
    let application = apply.into_payload();
    let evidence_octets = decode_hex(application.authorized_evidence_hex.payload());
    let evidence = rkyv::from_bytes::<Evidence, rkyv::rancor::Error>(&evidence_octets)
        .expect("the carried evidence decodes");
    (application.record_identifier.into_payload(), evidence)
}

/// Arm a spirit engine's quorum origination against `criome_socket` under the
/// admitted mirror contract, handing off to `stub`, with a tight completion
/// budget so the withhold case resolves quickly.
fn arm_quorum_origination(
    engine: &mut Engine,
    criome_socket: &Path,
    contract: ContractDigest,
    stub: &StubRouter,
) {
    engine.arm_criome_gate(criome_socket, SpiritAttestor::for_contract(contract));
    engine.arm_router_origination(RouterOrigination::new(
        stub.socket(),
        ActorIdentifier::new("spirit-a"),
        ActorIdentifier::new("spirit-b"),
    ));
    engine.set_quorum_completion_budget(QuorumCompletionBudget::new(
        Duration::from_secs(10),
        Duration::from_millis(50),
    ));
}

#[test]
fn a_committed_head_ships_only_on_the_gathered_2_of_2_quorum() {
    let runtime = runtime();
    let criome_a_dir = tempfile::tempdir().expect("criome a dir");
    let criome_b_dir = tempfile::tempdir().expect("criome b dir");
    let spirit_dir = tempfile::tempdir().expect("spirit dir");
    let router_dir = tempfile::tempdir().expect("router dir");

    let alpha = host("mirror-alpha");
    let beta = host("mirror-beta");
    let socket_a = criome_a_dir.path().join("alpha.sock");
    let socket_b = criome_b_dir.path().join("beta.sock");

    // Two independent criomes, each voiced to the other, mutually seeded, both
    // admitting the same content-addressed 2-of-2 mirror contract.
    let socket_a = spawn_criome_at(&criome_a_dir, "alpha", alpha.clone(), beta.clone(), socket_b.clone(), socket_a);
    let socket_b = spawn_criome_at(&criome_b_dir, "beta", beta.clone(), alpha.clone(), socket_a.clone(), socket_b);

    let key_a = node_public_key(&socket_a, alpha.clone());
    let key_b = node_public_key(&socket_b, beta.clone());
    register_peer(&socket_a, beta.clone(), key_b);
    register_peer(&socket_b, alpha.clone(), key_a);
    let contract = admit(&socket_a, mirror_contract(&alpha, &beta));
    let contract_b = admit(&socket_b, mirror_contract(&alpha, &beta));
    assert_eq!(contract, contract_b, "the 2-of-2 contract admits identically on both nodes");

    runtime.block_on(async move {
        let stub = StubRouter::bind(router_dir.path().join("router.sock"));
        let mut engine = open_spirit_engine(&spirit_dir, "spirit-a.sema");
        let record_identifier = record(&mut engine, "the mirrored change awaits a real quorum").await;
        arm_quorum_origination(&mut engine, &socket_a, contract, &stub);

        // The rewired seam PROPOSES the head; a 2-of-2 round opens Gathering, so a
        // detached ship awaits the peer's co-signature off the mailbox.
        let outcome = engine
            .gate_and_hand_to_router()
            .await
            .expect("the quorum boundary completes without machinery fault")
            .expect("a head exists to originate");
        assert_eq!(
            outcome,
            QuorumOriginationOutcome::Proposed,
            "the 2-of-2 round opens Gathering and a detached ship awaits completion",
        );

        // criome A solicited B across the voice, B co-signed, A assembled the real
        // 2-of-2 Evidence and authorized — and only THEN did Spirit ship.
        let payload = stub
            .captured()
            .expect("the head ships once the 2-of-2 quorum authorizes");
        let (shipped_identifier, evidence) = carried_evidence(&payload);
        assert_eq!(shipped_identifier, record_identifier, "the ship names the committed record");
        assert_eq!(
            evidence.signatures().len(),
            2,
            "the shipped Evidence carries BOTH members' operation signatures — a real 2-of-2 quorum, not a fabricated 1-of-1",
        );
        assert_eq!(
            evidence.stamp.signatures().len(),
            2,
            "the shared moment carries both members' time signatures",
        );
    });
}

#[test]
fn a_committed_head_is_withheld_while_the_peer_is_unreachable() {
    let runtime = runtime();
    let criome_a_dir = tempfile::tempdir().expect("criome a dir");
    let spirit_dir = tempfile::tempdir().expect("spirit dir");
    let router_dir = tempfile::tempdir().expect("router dir");

    let alpha = host("mirror-alpha");
    let beta = host("mirror-beta");
    let socket_a = criome_a_dir.path().join("alpha.sock");
    // A peer socket path that is never bound — the voice cannot reach it.
    let dead_socket = criome_a_dir.path().join("dead-beta.sock");
    let socket_a = spawn_criome_at(&criome_a_dir, "alpha", alpha.clone(), beta.clone(), dead_socket, socket_a);
    let contract = admit(&socket_a, mirror_contract(&alpha, &beta));

    runtime.block_on(async move {
        let stub = StubRouter::bind(router_dir.path().join("router.sock"));
        let mut engine = open_spirit_engine(&spirit_dir, "spirit-a.sema");
        record(&mut engine, "the mirrored change while the peer is down").await;
        arm_quorum_origination(&mut engine, &socket_a, contract, &stub);

        let outcome = engine
            .gate_and_hand_to_router()
            .await
            .expect("the quorum boundary completes without machinery fault")
            .expect("a head exists to originate");
        assert_eq!(
            outcome,
            QuorumOriginationOutcome::Proposed,
            "the round opens Gathering — the self-vote alone is one short of the 2-of-2 majority",
        );

        // The peer never co-signs; the round stays WITHHELD, the bounded ship
        // budget expires, and NOTHING crosses to the router — never last-writer-wins.
        assert!(
            stub.received_nothing(),
            "a head is withheld while the peer cannot co-sign; nothing hands off to the router",
        );
    });
}

/// Spawn a criome with a fixed socket path (so a mutual peer route can name it).
fn spawn_criome_at(
    directory: &TempDir,
    tag: &str,
    node: Identity,
    peer: Identity,
    peer_socket: PathBuf,
    socket: PathBuf,
) -> PathBuf {
    let store = StoreLocation::new(directory.path().join(format!("{tag}.sema")));
    let daemon = CriomeDaemon::new(socket.clone(), store)
        .with_node_identity(node)
        .with_quorum_voice(Arc::new(DirectDialQuorumVoice::new(vec![PeerSocketRoute::new(
            peer,
            peer_socket,
        )])));
    std::thread::spawn(move || {
        let _ = daemon.run();
    });
    wait_for_socket(&socket);
    socket
}
