//! THE OFFLINE FULL-CHAIN E2E WITNESS (designer report 669/2, P5 of 669).
//!
//! This is the first test that makes the WHOLE replication path true at once:
//!
//!   spirit source engine records intent
//!     → ships its versioned-log suffix to mirror A          (leg 1)
//!     → a router-carried object-accepted NOTICE crosses A→B  (leg 2)
//!     → mirror B fetches from mirror A and restores
//!       identically up to the head the notice announced.     (leg 3)
//!
//! It joins TWO already-green legs in ONE binary — no new shipped contract:
//!
//!   - Leg 1 + leg 3 are the ship → `ServerCommitted` → `Restore` →
//!     identical-records spine of `mirror`'s `tests/end_to_end_arc.rs`, run
//!     against a real in-process mirror `Service` over loopback TCP frames.
//!   - Leg 2 is the two-`RouterRuntime` loopback forward of `router`'s
//!     `tests/end_to_end_remote_forward.rs`: A submits → resolves the remote
//!     route → forwards over loopback TCP (offline accept-fixed verifier) → B
//!     verifies off-mailbox → delivers to a local `HarnessSocket` witness → A's
//!     trace reads `ForwardedRemote`.
//!
//! The ONE invention is the seam that makes the two legs CAUSAL rather than two
//! unrelated tests: `MirrorObjectNotice { store, head }`, a harness-local
//! payload that the router carries as a chat-shaped message body. It reuses
//! `signal-mirror`'s `HeadMark` shape (sequence + 32-byte digest), so promoting
//! it into a real `signal-router`/`signal-mirror` `MirrorObjectNotify` later is
//! mechanical. The e2e claim: an update recorded on A is, via a router-carried
//! notice, visible on B — mirror B fetches EXACTLY the head the router
//! announced, and asserts it restored up to that head.
//!
//! Fully offline: loopback TCP, no tailnet, no criome daemon. Crypto is the
//! router's offline accept-fixed verifier (D3).
//!
//! Mainline pin unification (operator handoff H4, report 672): the mirror and
//! router legs now come from their main branches. Their shared transitive crates
//! (triad-runtime, sema-engine, nota-next, signal-frame) also pin `branch=main`,
//! so Cargo unifies every shared crate onto one rev and the legs link in one
//! binary.

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kameo::actor::ActorRef;
use mirror::{
    ComponentShipper, Engine as MirrorEngine, MirrorTailnetClient, Service, ServiceLink,
    ShipOutcome, Store as MirrorStore,
};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use router::{
    Actor, ActorIdentifier, ApplyRouterObservation, ApplySignalMessage, ChannelLifetime,
    EndpointKind, EndpointTransport, GrantChannel, GrantRouteChannel, InstallRemotePeer,
    InstallRemoteRoute, ReadRouterTailnetAddress, ReadRouterTrace, RegisterActor,
    RemoteRouterIdentity, RouterInput, RouterNetworkConfiguration, RouterRuntime, RouterTraceStep,
    SignalMessageInput, TailnetAddress,
};
use sema_engine::{
    Assertion, CommitSequence, Durability, Engine as ComponentEngine, EngineOpen, EngineRecord,
    FamilyDirectory, FamilyName, MirrorHead, Mutation, PortableCheckpoint, QueryPlan, RecordKey,
    Retraction, RowMaterializer, SchemaHash, SchemaVersion, TableDescriptor, TableName,
    TableReference, VersionedCommitLogEntry, VersionedStoreName, VersioningPolicy,
};
use signal_frame::{NonEmpty, Reply, SubReply};
use signal_harness::{
    DeliveryCompleted, HarnessEvent, HarnessFrame, HarnessFrameBody, HarnessName, HarnessRequest,
};
use signal_message::{
    ConnectionClass as SignalConnectionClass, Input as SignalInput, MessageBody, MessageKind,
    MessageOrigin as SignalMessageOrigin, MessageRecipient, MessageSubmission,
    TimestampNanos as SignalTimestampNanos,
};
use signal_mirror::{
    Input as MirrorInput, Output as MirrorOutput, RestoreBundle, RestoreQuery, StoreName,
};
use signal_router::{
    Input as SignalRouterInput, MessageSlot, Output as SignalRouterOutput, RouterMessageTraceQuery,
};
use triad_runtime::kameo::actor::Spawn;

const COMPONENT_STORE_NAME: &str = "full-chain-witness";

// ---------------------------------------------------------------------------
// The seam: a harness-local object-accepted notice. The router carries this as
// a chat-shaped message body, NOTA-style — store + the confirmed HeadMark
// (sequence + digest). It is the seed of the future `MirrorObjectNotify`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct MirrorObjectNotice {
    store: String,
    sequence: u64,
    digest: [u8; 32],
}

impl MirrorObjectNotice {
    /// Build the notice from the head a ship confirmed.
    fn from_confirmed_head(store: &str, head: &MirrorHead) -> Self {
        Self {
            store: store.to_owned(),
            sequence: head.commit_sequence().value(),
            digest: *head.entry_digest().bytes(),
        }
    }

    /// Encode as a test-local NOTA record string for the router message body.
    /// Shape: `(MirrorObjectNotice <store> <sequence> <hex-digest>)`.
    fn to_nota(&self) -> String {
        format!(
            "(MirrorObjectNotice {} {} {})",
            self.store,
            self.sequence,
            Self::hex(&self.digest),
        )
    }

    /// Parse the notice back out of the delivered body.
    fn from_nota(body: &str) -> Self {
        let inner = body
            .trim()
            .strip_prefix("(MirrorObjectNotice ")
            .and_then(|rest| rest.strip_suffix(')'))
            .expect("notice body is a MirrorObjectNotice record");
        let mut fields = inner.split_whitespace();
        let store = fields.next().expect("notice carries a store").to_owned();
        let sequence = fields
            .next()
            .expect("notice carries a sequence")
            .parse::<u64>()
            .expect("sequence is an integer");
        let digest = Self::unhex(fields.next().expect("notice carries a digest"));
        assert!(fields.next().is_none(), "notice has exactly three fields");
        Self {
            store,
            sequence,
            digest,
        }
    }

    fn hex(bytes: &[u8; 32]) -> String {
        let mut text = String::with_capacity(64);
        for byte in bytes {
            text.push_str(&format!("{byte:02x}"));
        }
        text
    }

    fn unhex(text: &str) -> [u8; 32] {
        assert_eq!(text.len(), 64, "digest hex is 32 bytes");
        let mut bytes = [0_u8; 32];
        for (index, slot) in bytes.iter_mut().enumerate() {
            *slot =
                u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).expect("digest hex parses");
        }
        bytes
    }
}

// ---------------------------------------------------------------------------
// The component domain record (the mirror never decodes it) — verbatim from
// `end_to_end_arc.rs`. A stand-in for a spirit intent Entry; the mirror is
// payload-blind, so any EngineRecord proves the path.
// ---------------------------------------------------------------------------

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Debug, Clone, PartialEq, Eq)]
#[rkyv(derive(Debug))]
struct Thought {
    key: String,
    body: String,
}

impl Thought {
    fn new(key: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            body: body.into(),
        }
    }
}

impl EngineRecord for Thought {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.key.clone())
    }
}

struct Families {
    thoughts: TableReference<Thought>,
}

impl Families {
    fn new() -> Self {
        Self {
            thoughts: TableReference::new(TableName::new("thoughts")),
        }
    }
}

impl FamilyDirectory for Families {
    fn materialize(&self, row: RowMaterializer<'_>) -> sema_engine::Result<()> {
        match row.family().family().as_str() {
            "thought" => row.apply(self.thoughts),
            other => Err(sema_engine::Error::FamilyUnknown {
                family: other.to_owned(),
            }),
        }
    }
}

struct ComponentFixture {
    directory: tempfile::TempDir,
}

impl ComponentFixture {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(format!("{name}.sema"))
    }

    fn open_fresh(&self, file: &str) -> ComponentEngine {
        ComponentEngine::open(
            EngineOpen::new(self.path(file), SchemaVersion::new(1)).with_versioning(
                VersioningPolicy::new(VersionedStoreName::new(COMPONENT_STORE_NAME)),
            ),
        )
        .expect("component engine opens")
    }

    fn thought_descriptor(&self) -> TableDescriptor<Thought> {
        TableDescriptor::new(
            TableName::new("thoughts"),
            FamilyName::new("thought"),
            SchemaHash::for_label("thought-v1"),
        )
    }

    fn open_component(&self, file: &str) -> (ComponentEngine, TableReference<Thought>) {
        let mut engine = self.open_fresh(file);
        let thoughts = engine
            .register_table(self.thought_descriptor())
            .expect("thoughts register");
        (engine, thoughts)
    }

    /// Populate the spirit source: writes, a mid-history checkpoint, then
    /// post-checkpoint writes including a tombstone — same shape as
    /// `end_to_end_arc.rs::populate`, so the mirror restore carries a checkpoint
    /// body AND a live suffix.
    fn populate(&self) -> (ComponentEngine, TableReference<Thought>) {
        let (engine, thoughts) = self.open_component("component-source");
        engine
            .assert(Assertion::new(thoughts, Thought::new("alpha", "first")))
            .expect("assert alpha");
        engine
            .assert(Assertion::new(thoughts, Thought::new("beta", "second")))
            .expect("assert beta");
        engine
            .mutate(Mutation::new(thoughts, Thought::new("alpha", "revised")))
            .expect("mutate alpha");
        engine.checkpoint().expect("checkpoint writes");
        engine
            .assert(Assertion::new(thoughts, Thought::new("gamma", "third")))
            .expect("assert gamma");
        engine
            .retract(Retraction::new(thoughts, RecordKey::new("beta")))
            .expect("retract beta");
        (engine, thoughts)
    }
}

// ---------------------------------------------------------------------------
// Mirror A: a real in-process mirror Service over loopback TCP — the VC remote
// leg 1 ships into and leg 3 fetches from. Verbatim from `end_to_end_arc.rs`.
// ---------------------------------------------------------------------------

async fn running_mirror(directory: &tempfile::TempDir) -> (ServiceLink, SocketAddr) {
    let store =
        MirrorStore::open(&directory.path().join("mirror.sema")).expect("mirror store opens");
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
    (link, address)
}

// ---------------------------------------------------------------------------
// The notify witness on router B: a Unix socket that accepts one signal-harness
// delivery and reports it back to the test thread. Verbatim from
// `end_to_end_remote_forward.rs`.
// ---------------------------------------------------------------------------

struct HarnessWitness {
    path: std::path::PathBuf,
    received: Receiver<WitnessedDelivery>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WitnessedDelivery {
    harness: String,
    sender: String,
    body: String,
}

impl HarnessWitness {
    fn new(name: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "full-chain-e2e-{name}-{}-{now}.sock",
            std::process::id()
        ));
        let listener = UnixListener::bind(&path).expect("notify witness socket binds");
        let (sender, received) = channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("notify witness accepts delivery");
            let frame = read_harness_frame(&mut stream);
            let HarnessFrameBody::Request { exchange, request } = frame.into_body() else {
                panic!("expected harness request frame");
            };
            let HarnessRequest::MessageDelivery(delivery) = request.payloads().head().clone()
            else {
                panic!("expected message delivery request");
            };
            sender
                .send(WitnessedDelivery {
                    harness: delivery.harness.as_str().to_string(),
                    sender: delivery.sender.as_str().to_string(),
                    body: delivery.body.as_str().to_string(),
                })
                .expect("notify witness reports delivery");
            let reply = HarnessFrame::new(HarnessFrameBody::Reply {
                exchange,
                reply: Reply::committed(NonEmpty::single(SubReply::Ok(
                    HarnessEvent::DeliveryCompleted(DeliveryCompleted {
                        harness: HarnessName::new(delivery.harness.as_str()),
                        message_slot: delivery.message_slot,
                    }),
                ))),
            });
            stream
                .write_all(
                    reply
                        .encode_length_prefixed()
                        .expect("notify witness reply encodes")
                        .as_slice(),
                )
                .expect("notify witness writes reply");
            stream.flush().expect("notify witness flushes reply");
        });
        Self { path, received }
    }

    fn target(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn received(&self) -> WitnessedDelivery {
        self.received
            .recv_timeout(Duration::from_secs(5))
            .expect("notify witness receives delivery")
    }
}

impl Drop for HarnessWitness {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn read_harness_frame(stream: &mut impl Read) -> HarnessFrame {
    let mut prefix = [0_u8; 4];
    stream
        .read_exact(&mut prefix)
        .expect("notify witness reads frame prefix");
    let length = u32::from_be_bytes(prefix) as usize;
    let mut bytes = Vec::with_capacity(4 + length);
    bytes.extend_from_slice(&prefix);
    bytes.resize(4 + length, 0);
    stream
        .read_exact(&mut bytes[4..])
        .expect("notify witness reads frame body");
    HarnessFrame::decode_length_prefixed(bytes.as_slice()).expect("harness frame decodes")
}

async fn bound_tailnet_address(runtime: &ActorRef<RouterRuntime>) -> SocketAddr {
    runtime
        .ask(ReadRouterTailnetAddress)
        .await
        .expect("read tailnet bound address")
        .expect("the tailnet ingress is bound")
}

async fn apply_router_input(runtime: &ActorRef<RouterRuntime>, input: RouterInput) {
    runtime
        .ask(router::ApplyRouterInput { input })
        .await
        .expect("router input reaches runtime")
        .into_result()
        .expect("router input applies");
}

// ---------------------------------------------------------------------------
// Leg 3 import: fetch checkpoint + suffix from mirror A and import into a fresh
// component engine — the "own fetch transport." Verbatim shape from
// `end_to_end_arc.rs::Restorer::import`.
// ---------------------------------------------------------------------------

fn import_restore_bundle(bundle: RestoreBundle, target: &mut ComponentEngine) {
    let checkpoint =
        PortableCheckpoint::from_bytes(bundle.checkpoint.artifact.payload().payload().to_vec())
            .decode()
            .expect("decode checkpoint artifact");
    let suffix: Vec<VersionedCommitLogEntry> = bundle
        .suffix
        .iter()
        .map(|envelope| {
            rkyv::from_bytes::<VersionedCommitLogEntry, rkyv::rancor::Error>(
                envelope.payload.payload().payload(),
            )
            .expect("decode versioned entry payload")
        })
        .collect();
    let mut session = target.begin_import().expect("import session mints");
    session
        .ingest_checkpoint(checkpoint)
        .expect("checkpoint ingests");
    session.ingest_suffix(suffix);
    session
        .commit(&Families::new())
        .expect("import commits into the fresh store");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn intent_recorded_on_node_a_ships_notifies_over_router_and_restores_identically_on_node_b() {
    // === LEG 1: spirit source records intent → ship to mirror A ============
    let fixture = ComponentFixture::new();
    let (source, source_thoughts) = fixture.populate();

    let mirror_directory = tempfile::tempdir().expect("mirror temp dir");
    let (link, mirror_a_address) = running_mirror(&mirror_directory).await;

    // The owner registers the spirit store on mirror A's meta surface.
    let registered = link
        .meta(meta_signal_mirror::Input::RegisterStore(
            meta_signal_mirror::StoreRegistration::new(meta_signal_mirror::StoreName::new(
                COMPONENT_STORE_NAME.to_owned(),
            )),
        ))
        .await
        .expect("meta register");
    assert!(matches!(
        registered,
        meta_signal_mirror::Output::StoreRegistered(_)
    ));

    // Before shipping: the local history is queued for the mirror.
    assert_eq!(
        source.store_durability().expect("durability reads"),
        Durability::QueuedForMirror
    );

    // SHIP: outbox suffix → envelopes → real TCP frames → mirror persists →
    // acknowledged head → ServerCommitted. The production ComponentShipper.
    let shipper = ComponentShipper::new(
        source,
        mirror_a_address,
        VersionedStoreName::new(COMPONENT_STORE_NAME),
    );
    let confirmed_head = match shipper
        .ship_unshipped()
        .await
        .expect("shipper ships unshipped suffix")
    {
        ShipOutcome::Shipped { head } => head,
        other => panic!("expected shipped history, got {other:?}"),
    };
    assert_eq!(
        shipper
            .engine()
            .store_durability()
            .expect("durability reads"),
        Durability::ServerCommitted
    );
    assert_eq!(
        shipper
            .engine()
            .durability_of(confirmed_head.commit_sequence())
            .expect("per-entry durability reads"),
        Durability::ServerCommitted
    );
    assert!(
        shipper
            .engine()
            .unshipped_outbox()
            .expect("outbox reads")
            .is_empty(),
        "the shipped cursor covers the whole outbox"
    );

    // Publish the checkpoint the restorer fetches.
    let checkpoint_receipt = shipper
        .publish_latest_checkpoint()
        .await
        .expect("checkpoint publishes");
    assert_eq!(*checkpoint_receipt.sequence.payload(), 1);
    assert_eq!(*checkpoint_receipt.covered_end.payload(), 3);

    // Capture the notice the router will carry: store + the head mirror A
    // confirmed. THIS is the seam that makes legs 2 and 3 causal.
    let notice = MirrorObjectNotice::from_confirmed_head(COMPONENT_STORE_NAME, &confirmed_head);

    // === LEG 2: router A → router B carries the object-accepted notice =====
    let operator = ActorIdentifier::new("operator");
    // The submission enters router A as External(Owner); A stamps the sender as
    // "owner", and that is the `from` the forward carries to B — so B's channel
    // grant authorizes (owner → notify-target).
    let owner = ActorIdentifier::new("owner");
    let notify_target = ActorIdentifier::new("mirror-b-notify");
    let router_b_identity = RemoteRouterIdentity::new("router-b");

    // Router B: the notify witness (a local HarnessSocket), a listening tailnet
    // ingress, the notify-target registered LOCALLY, and the channel grant the
    // forwarded notice needs for B's channel-auth check.
    let witness = HarnessWitness::new("mirror-b");
    let router_b = RouterRuntime::start_networked(
        None,
        RouterNetworkConfiguration::offline_listening(
            "127.0.0.1:0".parse().expect("loopback address"),
            router_b_identity.clone(),
        ),
    )
    .await;
    let router_b_address = bound_tailnet_address(&router_b).await;

    apply_router_input(
        &router_b,
        RouterInput::RegisterActor(RegisterActor {
            actor: Actor {
                name: notify_target.clone(),
                pid: 7,
                endpoint: Some(EndpointTransport {
                    kind: EndpointKind::HarnessSocket,
                    target: witness.target(),
                    aux: None,
                }),
            },
        }),
    )
    .await;
    apply_router_input(
        &router_b,
        RouterInput::GrantChannel(GrantRouteChannel {
            channel: GrantChannel::direct_message(
                owner.clone(),
                notify_target.clone(),
                ChannelLifetime::Persistent,
            ),
        }),
    )
    .await;

    // Router A: a listening tailnet ingress (uniform actor-tree shape), router B
    // registered as a remote peer, and the notify-target's home set to router B.
    // The target has NO local registration on A.
    let router_a = RouterRuntime::start_networked(
        None,
        RouterNetworkConfiguration::offline_listening(
            "127.0.0.1:0".parse().expect("loopback address"),
            RemoteRouterIdentity::new("router-a"),
        ),
    )
    .await;
    let _router_a_address = bound_tailnet_address(&router_a).await;

    router_a
        .ask(InstallRemotePeer {
            identity: router_b_identity.clone(),
            address: TailnetAddress::new(router_b_address.to_string()),
        })
        .await
        .expect("install remote peer installs");
    router_a
        .ask(InstallRemoteRoute {
            recipient: notify_target.clone(),
            home: router_b_identity.clone(),
        })
        .await
        .expect("install remote route installs");

    // Submit the object-accepted NOTICE on router A: the body is the
    // NOTA-encoded coordinate of what landed on mirror A. A misses locally,
    // resolves the remote route, and forwards over loopback TCP to B.
    let accepted = router_a
        .ask(ApplySignalMessage {
            input: SignalMessageInput::with_origin(
                operator.clone(),
                SignalMessageOrigin::External(SignalConnectionClass::Owner),
                SignalInput::SubmitStamped(signal_message::StampedMessageSubmission {
                    submission: MessageSubmission {
                        recipient: MessageRecipient::new(notify_target.as_str().to_string()),
                        kind: MessageKind::Send,
                        body: MessageBody::new(notice.to_nota()),
                    },
                    origin: SignalMessageOrigin::External(SignalConnectionClass::Owner),
                    stamped_at: SignalTimestampNanos::new(1),
                }),
            ),
        })
        .await
        .expect("notice submission reaches router A")
        .into_result()
        .expect("router A accepts the notice submission");
    let signal_message::Output::SubmissionAccepted(acceptance) = accepted else {
        panic!("expected submission accepted, got {accepted:?}");
    };
    let submitted_slot = acceptance.into_payload().into_u64();

    // Router B delivered the notice to its LOCAL notify witness. The sender is
    // "owner": router A stamped the External(Owner) origin and that
    // authoritative sender rode the forward to B.
    let witnessed = witness.received();
    assert_eq!(witnessed.harness, "mirror-b-notify");
    assert_eq!(witnessed.sender, "owner");
    assert_eq!(witnessed.body, notice.to_nota());

    // THE SEAM: parse the notice back out and assert it equals the head mirror
    // A confirmed — the head announced over the router is exactly the head
    // mirror A committed.
    let delivered_notice = MirrorObjectNotice::from_nota(&witnessed.body);
    assert_eq!(delivered_notice, notice);

    // Router A's trace shows the notice left for a peer.
    let trace = router_a
        .ask(ReadRouterTrace { since: 0 })
        .await
        .expect("read router A trace")
        .into_result()
        .expect("router A trace is readable");
    assert!(
        trace
            .events()
            .iter()
            .any(|event| event.step() == RouterTraceStep::ForwardedRemote),
        "router A trace should record ForwardedRemote, got {:?}",
        trace
            .events()
            .iter()
            .map(|event| event.step())
            .collect::<Vec<_>>()
    );

    // Same fact through the typed observation surface.
    let trace_reply = router_a
        .ask(ApplyRouterObservation {
            request: SignalRouterInput::MessageTrace(RouterMessageTraceQuery {
                engine: signal_router::EngineIdentifier::new("router-a"),
                message_slot: MessageSlot::new(submitted_slot),
            }),
        })
        .await
        .expect("message trace query reaches router A")
        .into_result()
        .expect("message trace query answers");
    match trace_reply {
        SignalRouterOutput::MessageTrace(message_trace) => {
            assert_eq!(
                message_trace.status,
                signal_router::RouterDeliveryStatus::ForwardedRemote,
                "router A observation should report ForwardedRemote"
            );
        }
        other => panic!("expected MessageTrace reply, got {other:?}"),
    }

    // === LEG 3: mirror B fetch + restore, DRIVEN by the witnessed notice ===
    // Acting on the parsed notice, mirror B fetches from mirror A over its own
    // transport and restores into a fresh store.
    let restore_client = MirrorTailnetClient::new(mirror_a_address);
    let bundle = match restore_client
        .exchange(MirrorInput::Restore(RestoreQuery::new(StoreName::new(
            delivered_notice.store.clone(),
        ))))
        .await
        .expect("restore call succeeds")
    {
        MirrorOutput::Restored(bundle) => bundle,
        other => panic!("expected Restored, got {other:?}"),
    };
    assert_eq!(bundle.suffix.len(), 2, "gamma + the beta tombstone");

    let mut target = fixture.open_fresh("component-restored");
    import_restore_bundle(bundle, &mut target);
    let target_thoughts = target
        .register_table(fixture.thought_descriptor())
        .expect("thoughts re-register against the restored catalog");

    // THE CAUSAL SEAM: mirror B restored EXACTLY up to the head the router
    // announced.
    assert_eq!(
        target.current_commit_sequence().expect("target cursor"),
        CommitSequence::new(delivered_notice.sequence),
        "mirror B restored up to the head the router announced"
    );
    // And the announced digest matches the source's confirmed head digest.
    assert_eq!(
        delivered_notice.digest,
        *confirmed_head.entry_digest().bytes(),
    );

    // The normal query surface reads identical records on both engines.
    let source_records = shipper
        .engine()
        .match_records(QueryPlan::all(source_thoughts))
        .expect("source query")
        .records()
        .to_vec();
    let target_records = target
        .match_records(QueryPlan::all(target_thoughts))
        .expect("target query")
        .records()
        .to_vec();
    assert_eq!(source_records, target_records);
    assert_eq!(
        target_records,
        vec![
            Thought::new("alpha", "revised"),
            Thought::new("gamma", "third"),
        ]
    );

    // The restored store continues the same digest chain on B.
    assert_eq!(
        shipper
            .engine()
            .current_commit_sequence()
            .expect("source cursor"),
        target.current_commit_sequence().expect("target cursor"),
    );

    // Teardown: graceful shutdown of both routers (proven router-e2e teardown).
    let _ = router_a.stop_gracefully().await;
    router_a.wait_for_shutdown().await;
    let _ = router_b.stop_gracefully().await;
    router_b.wait_for_shutdown().await;
}
