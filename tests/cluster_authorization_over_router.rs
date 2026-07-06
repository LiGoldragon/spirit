//! THE CLUSTER-AUTHORIZATION LOOPCHECK (§3.7):
//! `spirit-cluster-authorizes-head-advance-over-router-test`.
//!
//! Real throughout, following the founding proof's node shape
//! (`router-two-hosts-found-root-over-router-test`): two node directories,
//! two REAL routers on loopback TCP with seeded pubkey-keyed host routes, two
//! REAL criome daemons whose conveyance originates over their local router,
//! extended with a REAL spirit engine on node A gated against criome A (the
//! cluster authorizer over the working socket) and an in-process mirror as
//! the ship target, plus a SHORT quorum window (seconds).
//!
//! Sequence and witnessed claims (batch, refusal, catch-up):
//!
//!   1. Found the 2-of-2 root over the router (the founding drive: initiate
//!      on A, explicit accepts on both meta sockets, Founded on both).
//!   2. BATCHED AUTHORIZED ADVANCE: record TWO entries on spirit A → drive
//!      the drain → ONE quorum round authorizes the batch head → the gate
//!      decision is Authorized with the authorized digest equal to the
//!      SECOND entry's digest, both entries ship (cursor at the head), and
//!      criome B holds the committed round for that ONE object — its ledger
//!      witnessed one round, not two.
//!   3. REFUSED ADVANCE (quorum can't complete): stop node B's router,
//!      record a third entry → drive the drain → the round stays Gathering,
//!      the window expires, Expired is pushed → the decision is a typed
//!      refusal, nothing ships, spirit A's LOCAL head still advanced (the
//!      local commit stands), and the outbox still holds the suffix.
//!   4. CATCH-UP RETRY AFTER REFUSAL: record a FOURTH entry (the head moves
//!      past the refused proposal), restore node B, drive the drain →
//!      criome A first completes the recorded round for the refused batch
//!      head, then authorizes the new head; the grant binds the fourth
//!      entry's digest; the whole suffix ships. (§3.4's no-wedge property,
//!      end to end.)
//!
//! Falsification: if spirit bypassed criome, step 3 would ship; if the parse
//! trusted status without the grant, a crafted Granted-without-grant criome
//! would ship (see tests/cluster_gate_session.rs); if the catch-up rule were
//! missing, step 4 would self-refuse with QuorumConflict and never grant.

mod support;

use support::domain_fixtures;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use criome::conveyance::{PeerActorRoute, RouterSubmission};
use criome::daemon::CriomeDaemon;
use criome::tables::StoreLocation;
use criome::transport::{CriomeClient, CriomeMetaClient};
use meta_signal_criome::{
    Input as CriomeMetaInput, Output as CriomeMetaOutput, RootFoundingAcceptance,
    RootFoundingInitiation, RootFoundingObservation, RootFoundingState, RootFoundingStatus,
};
use mirror::{Engine as MirrorEngine, Service, ServiceLink};
use router::{
    Actor, ActorIdentifier, ApplyMetaRouterPolicy, ApplyRoutedObjectSubmission, ApplyRouterInput,
    ChannelLifetime, CriomeHostId, EndpointKind, EndpointTransport, GrantChannel,
    GrantRouteChannel, InstallRemotePeer, InstallRemoteRoute, ReadRouterTailnetAddress,
    RegisterActor, RouterInput, RouterNetworkConfiguration, RouterRuntime, RouterTables,
    TailnetAddress,
};
use sema_engine::Durability;
use signal_criome::{
    BlsPublicKey, ComponentKind, Contract, CriomeReply, CriomeRequest, FoundingMember,
    GenesisDomainTag, Identity, NodePublicKeyObservation, ObjectDigest, PolicyMember,
    QuorumRoundIdentifier, QuorumRoundStatus, ReplayNonce, RequiredSignatureThreshold,
    RootAnchorDigest, RootGenesis, RoundPhase, Rule, Threshold,
};
use signal_frame::{NonEmpty, Reply, SubReply};
use signal_router::{
    Frame as SignalRouterFrame, FrameBody as SignalRouterFrameBody, Input as SignalRouterInput,
};
use spirit::schema::meta_signal::{
    ArchiveDatabaseTarget, ConfigureRequest, MirrorAddress, MirrorAddressText, MirrorTarget,
    Output as SpiritMetaOutput,
};
use spirit::schema::sema::RecordFamily;
use spirit::schema::signal::{
    Certainty, Description, Domains, Entry, Importance, Input, Justification, Kind, Magnitude,
    Output, Privacy, QuoteText, Reasoning, RecordRequest, Referent, Referents, Testimony,
    VerbatimQuote,
};
use spirit::{ClusterAuthorizer, CriomeAuthorization, Engine, GateDecision, GateRefusal, Store};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use triad_runtime::kameo::actor::{ActorRef, Spawn};
use triad_runtime::{FrameBody, LengthPrefixedCodec};

const STORE_NAME: &str = RecordFamily::STORE_NAME;
const CRIOME_ALPHA: &str = "criome-alpha";
const CRIOME_BETA: &str = "criome-beta";
/// Seconds in tests (the settled window posture): two rounds plus loopback
/// round-trips, and short enough that the refusal leg expires quickly.
const QUORUM_WINDOW: Duration = Duration::from_secs(5);

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after Unix epoch")
        .as_nanos()
}

struct NodePaths {
    working: PathBuf,
    meta: PathBuf,
    store: StoreLocation,
    router_socket: PathBuf,
    router_store: PathBuf,
}

impl NodePaths {
    fn new(tag: &str) -> Self {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "spirit-cluster-auth-{tag}-{}-{}",
            std::process::id(),
            nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create node fixture dir");
        let working = dir.join("criome.sock");
        Self {
            meta: working.with_file_name("criome.sock.meta"),
            store: StoreLocation::new(dir.join("criome.sema")),
            router_socket: dir.join("router.sock"),
            router_store: dir.join("router.sema"),
            working,
        }
    }
}

/// A working-socket front for one in-process router — the standing daemon's
/// origination path stood up around the runtime, exactly the founding proof's
/// shape, so the REAL `RouterSubmission` originates over a REAL router.
struct RouterWorkingSocket {
    _task: tokio::task::JoinHandle<()>,
}

impl RouterWorkingSocket {
    async fn bind(path: &Path, runtime: ActorRef<RouterRuntime>) -> Self {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).expect("router working socket binds");
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    return;
                };
                let runtime = runtime.clone();
                tokio::spawn(Self::serve_connection(stream, runtime));
            }
        });
        Self { _task: task }
    }

    async fn serve_connection(
        mut stream: tokio::net::UnixStream,
        runtime: ActorRef<RouterRuntime>,
    ) {
        let codec = LengthPrefixedCodec::default();
        let body = match codec.read_body_async(&mut stream).await {
            Ok(body) => body,
            Err(error) => {
                eprintln!("router front: request read failed: {error:?}");
                return;
            }
        };
        let frame = match SignalRouterFrame::decode(&body.into_bytes()) {
            Ok(frame) => frame,
            Err(error) => {
                eprintln!("router front: frame decode failed: {error:?}");
                return;
            }
        };
        let SignalRouterFrameBody::Request { exchange, request } = frame.into_body() else {
            eprintln!("router front: non-request frame");
            return;
        };
        let (input, _tail) = request.payloads.into_head_and_tail();
        let SignalRouterInput::SubmitRoutedObjects(submission) = input else {
            eprintln!("router front: unexpected router input");
            return;
        };
        let output = match runtime
            .ask(ApplyRoutedObjectSubmission { submission })
            .await
        {
            Ok(reply) => match reply.into_result() {
                Ok(output) => output,
                Err(error) => {
                    eprintln!("router front: submission refused by runtime: {error:?}");
                    return;
                }
            },
            Err(error) => {
                eprintln!("router front: runtime ask failed: {error:?}");
                return;
            }
        };
        let reply_frame = SignalRouterFrame::new(SignalRouterFrameBody::Reply {
            exchange,
            reply: Reply::committed(NonEmpty::single(SubReply::Ok(output))),
        });
        let Ok(bytes) = reply_frame.encode() else {
            return;
        };
        let _ = codec
            .write_body_async(&mut stream, &FrameBody::new(bytes))
            .await;
        let _ = stream.flush().await;
    }
}

fn host(name: &str) -> Identity {
    Identity::host(name.to_string())
}

fn start_criome(
    identity: Identity,
    paths: &NodePaths,
    source_actor_name: &str,
    peer: Identity,
    peer_destination_name: &str,
) {
    let conveyance = RouterSubmission::new(
        paths.router_socket.clone(),
        signal_router::ActorIdentifier::new(source_actor_name),
        vec![PeerActorRoute::new(
            peer,
            signal_router::ActorIdentifier::new(peer_destination_name),
        )],
    );
    let daemon = CriomeDaemon::new(paths.working.clone(), paths.store.clone())
        .with_node_identity(identity)
        .with_quorum_window(QUORUM_WINDOW)
        .with_peer_conveyance(Arc::new(conveyance));
    thread::spawn(move || {
        let _ = daemon.run();
    });
}

async fn wait_for_socket(socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(
            Instant::now() < deadline,
            "socket never appeared: {socket:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn node_public_key(socket: &Path) -> BlsPublicKey {
    let reply = CriomeClient::new(socket)
        .send(CriomeRequest::observe_node_public_key(
            NodePublicKeyObservation::new(),
        ))
        .unwrap_or_else(|error| panic!("observe node key on {socket:?}: {error}"));
    match reply {
        CriomeReply::NodePublicKey(key) => key.public_key().clone(),
        other => panic!("expected NodePublicKey, got {other:?}"),
    }
}

fn cohort(
    alpha: &Identity,
    beta: &Identity,
    key_alpha: &BlsPublicKey,
    key_beta: &BlsPublicKey,
) -> RootGenesis {
    let root_contract = Contract::root(Rule::threshold(Threshold::new(
        RequiredSignatureThreshold::new(2),
        vec![
            PolicyMember::key_member(alpha.clone()),
            PolicyMember::key_member(beta.clone()),
        ],
    )));
    RootGenesis::new(
        root_contract,
        vec![
            FoundingMember::new(alpha.clone(), key_alpha.clone()),
            FoundingMember::new(beta.clone(), key_beta.clone()),
        ],
        GenesisDomainTag::CriomeRootFoundingV1,
        ReplayNonce::new("cluster-authorization-loopcheck"),
    )
}

fn meta(socket: &Path, request: CriomeMetaInput) -> CriomeMetaOutput {
    CriomeMetaClient::new(socket)
        .send(request)
        .unwrap_or_else(|error| panic!("meta round-trip on {socket:?}: {error}"))
}

fn observe_status(socket: &Path) -> RootFoundingStatus {
    match meta(
        socket,
        CriomeMetaInput::ObserveRootFounding(RootFoundingObservation::new()),
    ) {
        CriomeMetaOutput::RootFoundingStatus(status) => status,
        other => panic!("expected RootFoundingStatus, got {other:?}"),
    }
}

fn accept(socket: &Path, anchor: &RootAnchorDigest, cohort: &RootGenesis) {
    match meta(
        socket,
        CriomeMetaInput::AcceptRootFounding(RootFoundingAcceptance::new(
            anchor.clone(),
            cohort.clone(),
        )),
    ) {
        CriomeMetaOutput::RootFoundingAccepted(_) => {}
        other => panic!("expected RootFoundingAccepted on {socket:?}, got {other:?}"),
    }
}

fn wait_until<Predicate>(what: &str, predicate: Predicate)
where
    Predicate: Fn() -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(30);
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(100));
    }
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
        .ask(ApplyRouterInput { input })
        .await
        .expect("router input reaches runtime")
        .into_result()
        .expect("router input applies");
}

async fn enable_mirror(runtime: &ActorRef<RouterRuntime>) {
    runtime
        .ask(ApplyMetaRouterPolicy {
            input: meta_signal_router::Input::set_mirror_enabled(
                meta_signal_router::MirrorEnabled::new(true),
            ),
        })
        .await
        .expect("meta SetMirrorEnabled reaches runtime")
        .into_result()
        .expect("SetMirrorEnabled applies");
}

async fn home_local_criome(
    router: &ActorRef<RouterRuntime>,
    local_actor: &ActorIdentifier,
    working_socket: &Path,
    peer_source: &ActorIdentifier,
) {
    apply_router_input(
        router,
        RouterInput::RegisterActor(RegisterActor {
            actor: Actor {
                name: local_actor.clone(),
                pid: 1,
                endpoint: Some(EndpointTransport {
                    kind: EndpointKind::ComponentSocket,
                    target: working_socket.to_string_lossy().into_owned(),
                    aux: None,
                }),
            },
        }),
    )
    .await;
    apply_router_input(
        router,
        RouterInput::GrantChannel(GrantRouteChannel {
            channel: GrantChannel::direct_message(
                peer_source.clone(),
                local_actor.clone(),
                ChannelLifetime::Persistent,
            ),
        }),
    )
    .await;
}

async fn seed_peer_route(
    router: &ActorRef<RouterRuntime>,
    peer_destination: &ActorIdentifier,
    peer_host_id: &CriomeHostId,
    peer_router_address: &TailnetAddress,
) {
    router
        .ask(InstallRemotePeer {
            identity: peer_host_id.clone(),
            address: peer_router_address.clone(),
        })
        .await
        .expect("install remote peer installs");
    router
        .ask(InstallRemoteRoute {
            recipient: peer_destination.clone(),
            home: peer_host_id.clone(),
        })
        .await
        .expect("install remote route installs");
}

/// Bring one node's router up (or back up) over its durable route store and
/// front it at the criome-visible working socket path.
async fn start_router(
    paths: &NodePaths,
    host_id: &CriomeHostId,
) -> (ActorRef<RouterRuntime>, TailnetAddress, RouterWorkingSocket) {
    let tables = RouterTables::open(&paths.router_store).expect("router tables open");
    let runtime = RouterRuntime::start_networked(
        Some(tables),
        RouterNetworkConfiguration::offline_listening(
            "127.0.0.1:0".parse().expect("loopback address"),
            host_id.clone(),
        ),
    )
    .await;
    let address = TailnetAddress::new(bound_tailnet_address(&runtime).await.to_string());
    let front = RouterWorkingSocket::bind(&paths.router_socket, runtime.clone()).await;
    enable_mirror(&runtime).await;
    (runtime, address, front)
}

fn record_request(description: &str) -> RecordRequest {
    RecordRequest {
        entry: Entry {
            domains: domain_fixtures::domains(&["Information/Documentation"]),
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

async fn running_mirror(directory: &Path) -> (ServiceLink, SocketAddr) {
    let store = mirror::Store::open(&directory.join("mirror.sema")).expect("mirror store opens");
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

/// The head digest as criome's `ObjectDigest` — the same projection the gate
/// applies (`ObjectDigest::from_bytes(head.bytes())`).
fn head_object_digest(engine: &Engine) -> ObjectDigest {
    let head = engine
        .versioned_log_head()
        .expect("versioned head reads")
        .expect("a committed head exists");
    ObjectDigest::from_bytes(head.bytes())
}

fn observed_round_status(socket: &Path, round: &QuorumRoundIdentifier) -> Option<QuorumRoundStatus> {
    match CriomeClient::new(socket)
        .send(CriomeRequest::observe_quorum_round(round.clone()))
        .expect("observe quorum round")
    {
        CriomeReply::QuorumRoundObserved(state) => Some(state.status),
        CriomeReply::Rejection(_) => None,
        other => panic!("expected QuorumRoundObserved or Rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn spirit_cluster_authorizes_head_advance_over_the_router() {
    let alpha = host("cluster-auth-alpha");
    let beta = host("cluster-auth-beta");
    let paths_a = NodePaths::new("alpha");
    let paths_b = NodePaths::new("beta");
    let criome_alpha = ActorIdentifier::new(CRIOME_ALPHA);
    let criome_beta = ActorIdentifier::new(CRIOME_BETA);

    // Criome-ready first, so the router identity gate resolves each real
    // Criome host ID (the master public key), never the sentinel.
    start_criome(
        alpha.clone(),
        &paths_a,
        CRIOME_ALPHA,
        beta.clone(),
        CRIOME_BETA,
    );
    start_criome(
        beta.clone(),
        &paths_b,
        CRIOME_BETA,
        alpha.clone(),
        CRIOME_ALPHA,
    );
    for socket in [
        &paths_a.working,
        &paths_a.meta,
        &paths_b.working,
        &paths_b.meta,
    ] {
        wait_for_socket(socket).await;
    }
    let key_a = {
        let working = paths_a.working.clone();
        tokio::task::spawn_blocking(move || node_public_key(&working))
            .await
            .expect("observe key task joins")
    };
    let key_b = {
        let working = paths_b.working.clone();
        tokio::task::spawn_blocking(move || node_public_key(&working))
            .await
            .expect("observe key task joins")
    };
    let host_id_a = RouterNetworkConfiguration::criome_host_id(&paths_a.working).await;
    let host_id_b = RouterNetworkConfiguration::criome_host_id(&paths_b.working).await;

    // Real routers over loopback TCP with durable route stores, fronted at
    // the criome-visible working sockets.
    let (router_a, address_a, _front_a) = start_router(&paths_a, &host_id_a).await;
    let (router_b, address_b, front_b) = start_router(&paths_b, &host_id_b).await;
    home_local_criome(&router_a, &criome_alpha, &paths_a.working, &criome_beta).await;
    home_local_criome(&router_b, &criome_beta, &paths_b.working, &criome_alpha).await;
    seed_peer_route(&router_a, &criome_beta, &host_id_b, &address_b).await;
    seed_peer_route(&router_b, &criome_alpha, &host_id_a, &address_a).await;

    // ── Step 1: found the 2-of-2 root over the router. ────────────────────
    let genesis = cohort(&alpha, &beta, &key_a, &key_b);
    let anchor = genesis.anchor().expect("cohort anchor");
    {
        let meta_a = paths_a.meta.clone();
        let meta_b = paths_b.meta.clone();
        let genesis = genesis.clone();
        let anchor = anchor.clone();
        tokio::task::spawn_blocking(move || {
            match meta(
                &meta_a,
                CriomeMetaInput::InitiateRootFounding(RootFoundingInitiation::new(genesis.clone())),
            ) {
                CriomeMetaOutput::RootFoundingStatus(status) => {
                    assert_eq!(status.state, RootFoundingState::Unfounded);
                }
                other => panic!("initiate must report status, got {other:?}"),
            }
            wait_until("the proposal to reach B over the router", || {
                observe_status(&meta_b)
                    .pending
                    .iter()
                    .any(|pending| pending.anchor == anchor)
            });
            accept(&meta_a, &anchor, &genesis);
            accept(&meta_b, &anchor, &genesis);
            wait_until("A to reach Founded over the router", || {
                observe_status(&meta_a).state == RootFoundingState::Founded
            });
            wait_until("B to reach Founded over the router", || {
                observe_status(&meta_b).state == RootFoundingState::Founded
            });
        })
        .await
        .expect("the founding drive completes");
    }

    // The spirit engine on node A: mirror armed (the ship target), gated
    // against criome A's working socket.
    let spirit_directory = tempfile::tempdir().expect("spirit temp dir");
    let (_mirror_link, mirror_address) = running_mirror(spirit_directory.path()).await;
    let store = Store::open(spirit_directory.path().join("spirit.sema")).expect("spirit store");
    let mut engine = Engine::new(store);
    engine.start().expect("spirit engine starts");
    let configured = engine.configure(ConfigureRequest::new(
        ArchiveDatabaseTarget::Default,
        Some(MirrorTarget::Address(MirrorAddress::new(
            MirrorAddressText::new(mirror_address.to_string()),
        ))),
        None,
        None,
    ));
    assert!(matches!(configured, SpiritMetaOutput::Configured(_)));
    engine.set_criome_authorization(CriomeAuthorization::Enabled(
        ClusterAuthorizer::new(&paths_a.working).with_session_deadline(Duration::from_secs(60)),
    ));

    // ── Step 2: the batched authorized advance. ────────────────────────────
    record(&mut engine, "batched entry one").await;
    record(&mut engine, "batched entry two").await;
    let first_entry_digest = {
        // The FIRST suffix entry's digest, for the one-round-not-two ledger
        // claim. (A record may commit auxiliary entries — referent
        // registrations — so the suffix holds AT LEAST the two records; the
        // batch head is the last entry either way.)
        let log = engine.store().versioned_log().expect("versioned log reads");
        assert!(log.len() >= 2, "both records committed locally");
        ObjectDigest::from_bytes(log[0].entry_digest().bytes())
    };
    let batch_head = head_object_digest(&engine);
    let decision = engine
        .drain_propagation_once()
        .await
        .expect("the drain completes without machinery fault")
        .expect("a head exists to authorize");
    match &decision {
        GateDecision::Authorized(reference) => {
            assert_eq!(
                reference.digest, batch_head,
                "the grant binds the batch head — the SECOND entry's digest"
            );
            assert_eq!(reference.component, ComponentKind::Spirit);
        }
        other => panic!("the founded 2-of-2 cluster authorizes the batch, got {other:?}"),
    }
    let handle = engine.store().engine_handle();
    assert_eq!(
        handle.store_durability().expect("durability reads"),
        Durability::ServerCommitted,
        "both entries shipped — the cursor is at the authorized batch head"
    );
    // Criome B's ledger witnessed ONE committed round, for the batch head —
    // and NO round for the first entry: one authorization covered the suffix.
    {
        let working_b = paths_b.working.clone();
        let batch_head = batch_head.clone();
        let first_entry_digest = first_entry_digest.clone();
        tokio::task::spawn_blocking(move || {
            let commit_round =
                QuorumRoundIdentifier::for_phase(&batch_head, RoundPhase::Commit);
            wait_until("criome B to hold the committed batch round", || {
                observed_round_status(&working_b, &commit_round)
                    == Some(QuorumRoundStatus::Authorized)
            });
            let first_entry_round =
                QuorumRoundIdentifier::for_phase(&first_entry_digest, RoundPhase::Request);
            assert_eq!(
                observed_round_status(&working_b, &first_entry_round),
                None,
                "criome B never saw a round for the first entry — one round covered the batch"
            );
        })
        .await
        .expect("criome B ledger assertions complete");
    }

    // ── Step 3: the refused advance — quorum can't complete. ──────────────
    let _ = router_b.stop_gracefully().await;
    router_b.wait_for_shutdown().await;
    drop(front_b);
    record(&mut engine, "an entry the dark cluster must refuse").await;
    let held_head = engine
        .versioned_log_head()
        .expect("versioned head reads")
        .expect("the local commit advanced the head");
    let refused = engine
        .drain_propagation_once()
        .await
        .expect("the drain completes without machinery fault")
        .expect("a head exists to authorize");
    assert_eq!(
        refused,
        GateDecision::Refused(GateRefusal::Expired),
        "the window expires fail-closed when the quorum cannot complete"
    );
    assert_eq!(
        engine
            .versioned_log_head()
            .expect("versioned head reads")
            .expect("the local head remains"),
        held_head,
        "the LOCAL commit stands — refusal holds propagation, never the store"
    );
    assert_eq!(
        handle.store_durability().expect("durability reads"),
        Durability::QueuedForMirror,
        "nothing shipped under the refusal"
    );
    assert!(
        !handle.unshipped_outbox().expect("outbox reads").is_empty(),
        "the refused suffix stays in the outbox"
    );

    // ── Step 4: catch-up retry after refusal. ─────────────────────────────
    record(&mut engine, "a fourth entry past the refused proposal").await;
    let fourth_head = head_object_digest(&engine);
    let (router_b, new_address_b, _front_b) = start_router(&paths_b, &host_id_b).await;
    home_local_criome(&router_b, &criome_beta, &paths_b.working, &criome_alpha).await;
    seed_peer_route(&router_b, &criome_alpha, &host_id_a, &address_a).await;
    seed_peer_route(&router_a, &criome_beta, &host_id_b, &new_address_b).await;

    let retried = engine
        .drain_propagation_once()
        .await
        .expect("the drain completes without machinery fault")
        .expect("a head exists to authorize");
    if !matches!(&retried, GateDecision::Authorized(_)) {
        // Post-mortem probes: what does each node hold for the third
        // (catch-up) and fourth heads' rounds?
        let third_head = ObjectDigest::from_bytes(held_head.bytes());
        for (label, socket) in [("A", &paths_a.working), ("B", &paths_b.working)] {
            for phase in [RoundPhase::Request, RoundPhase::Commit] {
                let round = QuorumRoundIdentifier::for_phase(&third_head, phase);
                let socket_clone = socket.clone();
                let round_clone = round.clone();
                let status = tokio::task::spawn_blocking(move || {
                    observed_round_status(&socket_clone, &round_clone)
                })
                .await
                .expect("probe joins");
                eprintln!("probe: node {label} {phase:?} round for THIRD head: {status:?}");
            }
        }
        for (label, socket) in [("A", &paths_a.working), ("B", &paths_b.working)] {
            for phase in [RoundPhase::Request, RoundPhase::Commit] {
                let round = QuorumRoundIdentifier::for_phase(&fourth_head, phase);
                let socket = socket.clone();
                let round_clone = round.clone();
                let status = tokio::task::spawn_blocking(move || {
                    observed_round_status(&socket, &round_clone)
                })
                .await
                .expect("probe joins");
                eprintln!("probe: node {label} {phase:?} round for fourth head: {status:?}");
            }
        }
    }
    match &retried {
        GateDecision::Authorized(reference) => {
            assert_eq!(
                reference.digest, fourth_head,
                "after catch-up the grant binds the FOURTH entry's digest"
            );
        }
        other => panic!(
            "the catch-up rule completes the recorded round then authorizes the new head, \
             got {other:?}"
        ),
    }
    assert_eq!(
        handle.store_durability().expect("durability reads"),
        Durability::ServerCommitted,
        "the whole suffix shipped after the catch-up grant"
    );
    assert!(
        handle.unshipped_outbox().expect("outbox reads").is_empty(),
        "the outbox drained to the authorized head"
    );

    let _ = router_b.stop_gracefully().await;
    router_b.wait_for_shutdown().await;
    let _ = router_a.stop_gracefully().await;
    router_a.wait_for_shutdown().await;
}
