//! THE EVERYWHERE-GATE INTAKE WITNESSES (§3.5, §3.8, §3.9).
//!
//! With criome authorization Enabled, a head-advancing working operation is
//! STAGED — built and durably parked without committing — and accepted ONLY
//! on the cluster grant; every other terminal verdict refuses the operation
//! to the caller (`Output::AdvanceRefused`) and discards the staged group,
//! so nothing is recorded anywhere: the head does not advance, not even
//! locally. Reads are unaffected.
//!
//! Falsification lines: if spirit bypassed criome, the refused legs would
//! accept and advance the head; if staging leaked, a refused operation would
//! leave an observable trace (count, head, outbox, queries); if the grant
//! were not the acceptance event, the granted leg would differ from the
//! Disabled-mode twin byte-for-byte; if recovery discarded a granted group,
//! the crash leg's cluster-accepted digest would name entries nobody holds.

mod support;

use std::io::BufReader;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::thread;

use criome::transport::CriomeFrameCodec;
use signal_criome::{
    AuthorizationGrant, AuthorizationObservationSnapshot, AuthorizationPolicyClass,
    AuthorizationPolicySatisfaction, AuthorizationRequestSlot, AuthorizationStateRecord,
    AuthorizationStatus, CriomeReply, CriomeRequest, Identity, ObjectDigest,
    SignatureAuthorizationResult, TimestampNanos,
};
use spirit::schema::daemon::{ComponentDaemon, WorkingInputLane};
use spirit::schema::signal::{
    AdvanceRefusalReason, Description, Entry, Importance, ImportanceBump, Input, Justification,
    Kind, Magnitude, Output, QuoteText, Reasoning, RecordIdentifier, RecordRequest, SearchText,
    Statement, StatementText, Testimony, VerbatimQuote,
};
use spirit::{
    ClusterAuthorizer, CriomeAuthorization, Engine, SPIRIT_STORE_NAME, SpiritDaemon, StagedIntake,
    Store,
};
use support::domain_fixtures;

const _STORE_NAME: &str = SPIRIT_STORE_NAME;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn record_request(description: &str) -> RecordRequest {
    RecordRequest {
        entry: Entry {
            domains: domain_fixtures::domains(&["Information/Documentation"]),
            kind: Kind::Decision,
            description: Description::new(description),
            importance: Importance::new(Magnitude::Medium),
        },
        justification: Justification {
            testimony: Testimony::new(vec![VerbatimQuote::new(QuoteText::new(description), None)]),
            reasoning: Reasoning::new(description),
        },
    }
}

fn open_engine(path: PathBuf) -> Engine {
    let store = Store::open(path).expect("open spirit store");
    let mut engine = Engine::new(store);
    engine.start().expect("engine starts");
    engine
}

/// One observable-state snapshot: everything a reader could see. A refused
/// operation must leave every one of these untouched.
#[derive(Debug, PartialEq, Eq)]
struct ObservableState {
    record_count: usize,
    versioned_head: Option<sema_engine::EntryDigest>,
    unshipped_outbox_length: usize,
}

impl ObservableState {
    fn capture(engine: &Engine) -> Self {
        let handle = engine.store().engine_handle();
        Self {
            record_count: engine.record_count(),
            versioned_head: engine.versioned_log_head().expect("head reads"),
            unshipped_outbox_length: handle.unshipped_outbox().expect("outbox reads").len(),
        }
    }
}

/// THE CLOSED INTAKE CLASSIFICATION (§3.5.5): representatives of both lanes.
/// The exhaustive match itself lives in `SpiritDaemon::working_input_lane`,
/// so a new `Input` variant fails the COMPILE until it chooses a lane; this
/// witness pins the semantic of each class.
#[test]
fn head_advancing_inputs_stage_and_reads_stay_immediate() {
    let advancing: Vec<Input> = vec![
        Input::record(record_request("a lane witness record")),
        Input::state(Statement::new(StatementText::new("a raw statement"))),
        Input::bump_importance(ImportanceBump::new(RecordIdentifier::new("record-1"))),
        Input::retire(spirit::schema::signal::Retirement {
            record_identifier: RecordIdentifier::new("record-1"),
            justification: record_request("justify").justification,
        }),
    ];
    for input in &advancing {
        assert_eq!(
            SpiritDaemon::working_input_lane(input),
            WorkingInputLane::Staged,
            "a head-advancing input takes the staged lane: {input:?}"
        );
    }
    let immediate: Vec<Input> = vec![
        Input::Version,
        Input::Marker,
        Input::text_search(SearchText::new("anything")),
    ];
    for input in &immediate {
        assert_eq!(
            SpiritDaemon::working_input_lane(input),
            WorkingInputLane::Immediate,
            "a read passes ungated on the immediate lane: {input:?}"
        );
    }
}

/// Reads are unaffected (§0): with the gate Enabled and NO reachable criome,
/// reads complete normally — they never wait on a round and are never
/// authorized.
#[test]
fn reads_flow_ungated_while_the_gate_is_enabled_and_criome_is_absent() {
    let runtime = runtime();
    let directory = tempfile::tempdir().expect("tempdir");
    runtime.block_on(async {
        let mut engine = open_engine(directory.path().join("reads.sema"));
        // Seed one record while Disabled so the observation read has content.
        let seeded = engine
            .handle_async(Input::record(record_request("a seeded record to observe")))
            .await
            .into_root();
        assert!(matches!(seeded, Output::RecordAccepted(_)));
        engine.set_criome_authorization(CriomeAuthorization::Enabled(ClusterAuthorizer::new(
            directory.path().join("no-criome.sock"),
        )));
        let version = engine.handle_async(Input::Version).await.into_root();
        assert!(
            matches!(version, Output::VersionReported(_)),
            "reads stay served under an enabled gate, got {version:?}"
        );
        let search = engine
            .handle_async(Input::text_search(SearchText::new("seeded")))
            .await
            .into_root();
        assert!(
            matches!(search, Output::RecordsObserved(_)),
            "observation reads stay served under an enabled gate, got {search:?}"
        );
    });
}

/// THE CORRECTED OUTCOME (§3.9 step 4 in unit form): an unreachable criome
/// refuses the OPERATION — the caller receives `AdvanceRefused(Unreachable)`,
/// the head does NOT advance, the record count is unchanged, no outbox trace
/// exists, and the staging slot is clear. Fail-closed, nothing recorded
/// anywhere.
#[test]
fn a_refused_advance_is_refused_to_the_caller_and_leaves_no_trace() {
    let runtime = runtime();
    let directory = tempfile::tempdir().expect("tempdir");
    runtime.block_on(async {
        let mut engine = open_engine(directory.path().join("refused.sema"));
        engine.set_criome_authorization(CriomeAuthorization::Enabled(
            ClusterAuthorizer::new(directory.path().join("no-criome.sock"))
                .with_session_deadline(std::time::Duration::from_secs(1)),
        ));
        let before = ObservableState::capture(&engine);

        let staged = engine
            .stage_working_input(Input::record(record_request("a refused operation")))
            .await;
        let StagedIntake::Parked(mut advance) = staged else {
            panic!("a head advance under an enabled gate parks, got {staged:?}");
        };
        advance.resolve().await;
        let reply = engine.conclude_staged_advance(advance).await;
        match reply {
            Output::AdvanceRefused(refused) => assert_eq!(
                refused.payload().payload(),
                &AdvanceRefusalReason::Unreachable,
                "an absent criome refuses the operation as Unreachable"
            ),
            other => panic!("the operation is refused to the caller, got {other:?}"),
        }

        assert_eq!(
            ObservableState::capture(&engine),
            before,
            "a refused operation leaves no observable trace"
        );
        assert!(
            engine
                .store()
                .engine_handle()
                .staged_group()
                .expect("slot reads")
                .is_none(),
            "the staging slot never survives a refusal"
        );
    });
}

/// The stub criome: accepts connections and answers each ask with ONE
/// scripted snapshot — a terminal state bound to the submitted digest under
/// a stub-assigned slot (the session token IS that slot).
struct StubCriome {
    _directory: tempfile::TempDir,
    socket_path: PathBuf,
    _thread: thread::JoinHandle<()>,
}

#[derive(Clone, Copy)]
enum StubVerdict {
    Granted,
    Denied,
    Expired,
    Unavailable,
}

impl StubCriome {
    fn spawn(verdict: StubVerdict, accepts: usize) -> Self {
        let directory = tempfile::tempdir().expect("stub criome tempdir");
        let socket_path = directory.path().join("criome.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind stub criome socket");
        let thread = thread::spawn(move || {
            for _connection in 0..accepts {
                let Ok((stream, _peer)) = listener.accept() else {
                    return;
                };
                let mut stream = BufReader::new(stream);
                let codec = CriomeFrameCodec::default();
                let Ok(request) = codec.read_request(&mut stream) else {
                    return;
                };
                let CriomeRequest::AuthorizeSignalCall(authorization) = request else {
                    panic!("expected AuthorizeSignalCall at the stub");
                };
                let digest = authorization
                    .authorized_object_reference
                    .object_digest
                    .clone();
                let slot = AuthorizationRequestSlot::new("stub-slot");
                let (status, grant) = match verdict {
                    StubVerdict::Granted => (
                        AuthorizationStatus::Granted,
                        Some(Self::grant(slot.clone(), digest.clone())),
                    ),
                    StubVerdict::Denied => (AuthorizationStatus::Denied, None),
                    StubVerdict::Expired => (AuthorizationStatus::Expired, None),
                    StubVerdict::Unavailable => (AuthorizationStatus::Unavailable, None),
                };
                let snapshot = AuthorizationObservationSnapshot::from_states(vec![
                    AuthorizationStateRecord::new(slot, digest, status, Vec::new(), grant, None),
                ]);
                let _ = codec.write_reply(
                    stream.get_mut(),
                    CriomeReply::AuthorizationObservationSnapshot(snapshot),
                );
            }
        });
        Self {
            _directory: directory,
            socket_path,
            _thread: thread,
        }
    }

    fn grant(slot: AuthorizationRequestSlot, digest: ObjectDigest) -> AuthorizationGrant {
        AuthorizationGrant::new(
            slot,
            signal_criome::AuthorizedObjectReference {
                component_kind: signal_criome::ComponentKind::Spirit,
                object_digest: digest,
                authorized_object_kind: signal_criome::AuthorizedObjectKind::Head,
            },
            AuthorizationPolicySatisfaction::new(
                AuthorizationPolicyClass::ComplexQuorum,
                signal_criome::RequiredSignatureThreshold::new(2),
                vec![Identity::host("stub-node".to_owned())],
            ),
            SignatureAuthorizationResult::RequiredSignaturesSatisfied,
            Vec::new(),
            Identity::host("stub-node".to_owned()),
            TimestampNanos::new(1),
            None,
        )
    }

    fn authorizer(&self) -> ClusterAuthorizer {
        ClusterAuthorizer::new(&self.socket_path)
            .with_session_deadline(std::time::Duration::from_secs(5))
    }
}

/// THE GRANT IS THE ACCEPTANCE EVENT (§0.1): a granted advance materializes
/// exactly the staged group — the log head equals the digest the cluster
/// granted, the held accepted reply is released only after the verdict, and
/// the store content equals the Disabled-mode twin's. (Record identifiers
/// are mint-random per store, so cross-store equality is content equality;
/// the staged group carries its identifiers durably, and the crash witness
/// pins that stage → materialize — and recovery — reproduce them exactly.)
#[test]
fn a_granted_advance_materializes_exactly_the_staged_group() {
    let runtime = runtime();
    let directory = tempfile::tempdir().expect("tempdir");
    runtime.block_on(async {
        // The Disabled twin: today's direct write, for content equality.
        let mut twin = open_engine(directory.path().join("twin.sema"));
        let twin_reply = twin
            .handle_async(Input::record(record_request("the granted operation")))
            .await
            .into_root();
        let Output::RecordAccepted(twin_accepted) = &twin_reply else {
            panic!("the twin accepts directly, got {twin_reply:?}");
        };
        let twin_entry = twin
            .store()
            .entry_by_identifier(twin_accepted.payload().payload())
            .expect("twin lookup")
            .expect("the twin record exists");

        // The gated engine: stage, authorize against the granting stub,
        // materialize on the grant.
        let stub = StubCriome::spawn(StubVerdict::Granted, 1);
        let mut engine = open_engine(directory.path().join("gated.sema"));
        engine.set_criome_authorization(CriomeAuthorization::Enabled(stub.authorizer()));

        let staged = engine
            .stage_working_input(Input::record(record_request("the granted operation")))
            .await;
        let StagedIntake::Parked(mut advance) = staged else {
            panic!("the advance parks awaiting the round, got {staged:?}");
        };
        let parked_digest = advance.prospective_head().clone();
        assert_eq!(
            engine.versioned_log_head().expect("head reads"),
            None,
            "nothing is committed while the round runs"
        );
        advance.resolve().await;
        let reply = engine.conclude_staged_advance(advance).await;
        let Output::RecordAccepted(gated_accepted) = &reply else {
            panic!("the grant releases the held accepted reply, got {reply:?}");
        };
        let gated_head = engine
            .versioned_log_head()
            .expect("head reads")
            .expect("the grant materialized the group");
        assert_eq!(
            gated_head, parked_digest,
            "the materialized head equals the digest the cluster granted"
        );
        // The held reply's identifier resolves to the recorded entry, and
        // the entry content equals the twin's — acceptance produced the same
        // record either way.
        let gated_entry = engine
            .store()
            .entry_by_identifier(gated_accepted.payload().payload())
            .expect("gated lookup")
            .expect("the held reply's identifier resolves after materialization");
        assert_eq!(
            gated_entry, twin_entry,
            "entry content equals the Disabled twin's"
        );
        assert_eq!(engine.record_count(), twin.record_count());
        assert!(
            engine
                .store()
                .engine_handle()
                .staged_group()
                .expect("slot reads")
                .is_none(),
            "materialization clears the staging slot"
        );
    });
}

/// §3.5.2: each terminal refusal reason surfaces to the caller as its own
/// closed `AdvanceRefusalReason` — no strings, no collapse.
#[test]
fn each_terminal_refusal_reason_surfaces_to_the_caller() {
    let runtime = runtime();
    for (verdict, reason) in [
        (StubVerdict::Denied, AdvanceRefusalReason::Denied),
        (StubVerdict::Expired, AdvanceRefusalReason::Expired),
        (StubVerdict::Unavailable, AdvanceRefusalReason::Unavailable),
    ] {
        let directory = tempfile::tempdir().expect("tempdir");
        runtime.block_on(async {
            let stub = StubCriome::spawn(verdict, 1);
            let mut engine = open_engine(directory.path().join("reasons.sema"));
            engine.set_criome_authorization(CriomeAuthorization::Enabled(stub.authorizer()));
            let before = ObservableState::capture(&engine);
            let staged = engine
                .stage_working_input(Input::record(record_request("a refused reason probe")))
                .await;
            let StagedIntake::Parked(mut advance) = staged else {
                panic!("the advance parks, got {staged:?}");
            };
            advance.resolve().await;
            let reply = engine.conclude_staged_advance(advance).await;
            match reply {
                Output::AdvanceRefused(refused) => assert_eq!(
                    refused.payload().payload(),
                    &reason,
                    "the terminal verdict surfaces its own closed reason"
                ),
                other => panic!("the operation is refused to the caller, got {other:?}"),
            }
            assert_eq!(
                ObservableState::capture(&engine),
                before,
                "a refusal leaves no observable trace"
            );
        });
    }
}

/// §3.8 CRASH RECOVERY (cases 1 and 3): a staged group parked when the
/// process dies is resolved by a recovery re-ask BEFORE new advances are
/// accepted — an occupied slot refuses new head advances fail-closed, the
/// recovery grant materializes the parked group (case 3: once granted, the
/// operation IS accepted and materialization MUST complete), and the store
/// afterwards equals the never-crashed twin.
#[test]
fn crash_recovery_materializes_a_parked_group_on_the_recovery_grant() {
    let runtime = runtime();
    let directory = tempfile::tempdir().expect("tempdir");
    let store_path = directory.path().join("crash.sema");
    let parked_digest = runtime.block_on(async {
        let stub = StubCriome::spawn(StubVerdict::Granted, 0);
        let mut engine = open_engine(store_path.clone());
        engine.set_criome_authorization(CriomeAuthorization::Enabled(stub.authorizer()));
        let staged = engine
            .stage_working_input(Input::record(record_request("the crash-window operation")))
            .await;
        let StagedIntake::Parked(advance) = staged else {
            panic!("the advance parks, got {staged:?}");
        };
        // CRASH between park and conclude: the advance is dropped without a
        // verdict, the engine is dropped, the durable slot survives.
        advance.prospective_head().clone()
    });

    runtime.block_on(async {
        let mut engine = open_engine(store_path.clone());
        let occupied = engine
            .store()
            .engine_handle()
            .staged_group()
            .expect("slot reads")
            .expect("the parked group survived the crash");
        assert_eq!(
            occupied.prospective_head(),
            parked_digest,
            "the slot carries the parked prospective digest"
        );

        // An occupied, unresolved slot refuses every new head advance —
        // fail-closed until recovery resolves it.
        let stub = StubCriome::spawn(StubVerdict::Granted, 1);
        engine.set_criome_authorization(CriomeAuthorization::Enabled(stub.authorizer()));
        let refused = engine
            .stage_working_input(Input::record(record_request("too early")))
            .await;
        match refused {
            StagedIntake::Completed(Output::AdvanceRefused(refused)) => assert_eq!(
                refused.payload().payload(),
                &AdvanceRefusalReason::Unavailable,
                "an occupied slot refuses new advances as Unavailable"
            ),
            other => panic!("new advances refuse until recovery, got {other:?}"),
        }

        // Recovery: the re-ask reaches the (granting) criome — from its view
        // a first ask or the standing re-grant — and materializes the group.
        engine.resolve_parked_staged_group().await;
        assert!(
            engine
                .store()
                .engine_handle()
                .staged_group()
                .expect("slot reads")
                .is_none(),
            "recovery resolved the slot"
        );
        assert_eq!(
            engine
                .versioned_log_head()
                .expect("head reads")
                .expect("recovery materialized the parked group"),
            parked_digest,
            "the materialized head equals the cluster-accepted digest"
        );
        assert_eq!(
            engine.record_count(),
            1,
            "the operation exists exactly once"
        );
    });
}
