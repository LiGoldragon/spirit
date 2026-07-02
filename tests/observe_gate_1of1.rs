//! The criome-gate-only OBSERVE witness (Spirit `xhwa`, om4g.2; audit M1).
//!
//! This closes the gap the auditor flagged: the shipped-daemon observe path —
//! the `cfg(all(criome-gate, not(mirror-shipper)))` dispatch inside
//! `SpiritDaemon::handle_working_input` — had NO behavioral test. The only other
//! criome-gate witness (`criome_gate_1of1`) requires `mirror-shipper` and
//! exercises a DIFFERENT method (`gate_and_ship_head`); `observe_gate_head` and
//! the non-overlapping daemon dispatch that calls it were only compile-checked.
//!
//! Both proofs drive the REAL daemon boundary — `handle_working_input`, the same
//! entry the running daemon calls per accepted working connection — in a build
//! WITHOUT the mirror shipper. The gate's `CriomeGate::observe_authorization`
//! does a genuine Unix-socket round-trip to a live local criome daemon
//! (`serve_forever` on its own OS thread), not an in-process ask.
//!
//!   (a) ARMED gate — an owner meta `Configure(CriomeGateTarget::Socket)` arms
//!       the gate against the local criome (AutoApprove). A working record driven
//!       through the daemon observes the post-commit head, criome answers
//!       `Observed`, and the daemon path emits exactly one
//!       `AuthorizationObjectName::Observed` trace event (om4g.1) WITHOUT
//!       shipping — this build has no mirror.
//!
//!   (b) UNARMED gate — no `Configure` yet. The same working record drives the
//!       same dispatch, `observe_gate_head` returns `Ok(None)` (a no-op), and NO
//!       authorization trace event is emitted.
//!
//! Falsification: if the daemon skipped the observe dispatch, the armed case
//! would emit no `Observed` event; if an unarmed gate spuriously observed, the
//! unarmed case would emit one.

use std::net::SocketAddr;

use criome::daemon::CriomeDaemon;
use criome::tables::StoreLocation;
use signal_criome::AuthorizationMode as CriomeAuthorizationMode;
use spirit::schema::meta_signal::{
    ArchiveDatabaseTarget, ConfigureRequest, CriomeGateTarget, CriomeSocketPathText,
    Output as MetaOutput,
};
use spirit::schema::signal::{
    Certainty, Description, Domains, Entry, Importance, Input, Justification, Kind, Magnitude,
    Output, Privacy, QuoteText, Reasoning, RecordRequest, Referent, Referents, Testimony,
    VerbatimQuote,
};
use spirit::{
    AuthorizationObjectName, ComponentDaemon, Engine, ObjectName, SpiritDaemon, Store, TraceEvent,
    TraceLog,
};
use tempfile::TempDir;
use triad_runtime::ConnectionContext;

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

fn criome_gate_target(path: &std::path::Path) -> CriomeGateTarget {
    CriomeGateTarget::socket(CriomeSocketPathText::new(path.display().to_string()))
}

/// Run a real local criome daemon in AutoApprove mode over a fresh Unix socket
/// on its own OS thread. `bind()` creates the socket file before returning, so
/// the gate's client finds it. The owned temp dir keeps the path alive.
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

/// A loopback connection context. The observe dispatch classifies no origin
/// from the transport, so `handle_working_input` takes it as `_connection`; the
/// witness supplies a real one so it drives the exact production boundary.
fn loopback_connection() -> ConnectionContext {
    ConnectionContext::from(
        "127.0.0.1:0"
            .parse::<SocketAddr>()
            .expect("loopback address"),
    )
}

/// A fresh trace-recording spirit engine, started, with no criome configured.
fn started_engine(directory: &TempDir, trace_log: TraceLog) -> Engine {
    let store = Store::open(directory.path().join("source.sema")).expect("open spirit store");
    let mut engine = Engine::new_with_trace(store, trace_log);
    engine.start().expect("engine starts");
    engine
}

/// How many `AuthorizationObserved` trace events the recorded stream carries.
fn observed_authorization_count(events: &[TraceEvent]) -> usize {
    events
        .iter()
        .filter(|event| {
            event.object_name() == ObjectName::Authorization(AuthorizationObjectName::Observed)
        })
        .count()
}

#[test]
fn armed_gate_observes_head_and_emits_authorization_trace_through_daemon() {
    let runtime = runtime();
    let criome_directory = tempfile::tempdir().expect("criome temp dir");
    let component_directory = tempfile::tempdir().expect("component temp dir");
    let criome_socket = spawn_auto_approve_criome(&criome_directory);

    let trace_log = TraceLog::recording();
    let mut engine = started_engine(&component_directory, trace_log.clone());

    // Owner Configure arms the gate against the live local criome socket — the
    // deployed daemon's bootstrap arming path (om4g.2). No mirror target: this
    // build has no shipper to configure.
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
        "meta Configure arms the criome gate"
    );

    runtime.block_on(async {
        let connection = loopback_connection();
        let output = SpiritDaemon::handle_working_input(
            &mut engine,
            Input::record(record_request("the armed observe head rides the auth watch")),
            &connection,
        )
        .await
        .expect("the daemon handles the working input without fault");
        assert!(
            matches!(output, Output::RecordAccepted(_)),
            "the working record is accepted, got {output:?}"
        );
    });

    // The armed daemon dispatch observed the post-commit head over the criome
    // socket and emitted the authorization trace event — exactly once for the
    // one committed head.
    assert_eq!(
        observed_authorization_count(&trace_log.events()),
        1,
        "the armed daemon observe dispatch emits one AuthorizationObserved trace event"
    );
}

#[test]
fn unarmed_gate_is_a_noop_through_daemon() {
    let runtime = runtime();
    let component_directory = tempfile::tempdir().expect("component temp dir");

    let trace_log = TraceLog::recording();
    let mut engine = started_engine(&component_directory, trace_log.clone());
    assert!(
        !engine.criome_gate_armed(),
        "the criome gate is unarmed with no Configure"
    );

    runtime.block_on(async {
        let connection = loopback_connection();
        let output = SpiritDaemon::handle_working_input(
            &mut engine,
            Input::record(record_request("the unarmed observe path is a no-op")),
            &connection,
        )
        .await
        .expect("the daemon handles the working input without fault");
        assert!(
            matches!(output, Output::RecordAccepted(_)),
            "the working record is accepted, got {output:?}"
        );
    });

    // An unarmed gate attempts no observation: `observe_gate_head` returns
    // `Ok(None)`, so no authorization trace event rides the watch.
    assert_eq!(
        observed_authorization_count(&trace_log.events()),
        0,
        "an unarmed gate observes nothing — no AuthorizationObserved trace event"
    );
}
