//! Owner-only meta `Configure` route end-to-end.
//!
//! Proves the meta-signal listener wiring: a `Configure` request routes through
//! the owner-only meta socket, applies the configuration effect (re-points the
//! durable archive target the working SEMA path writes to), and replies with a
//! typed receipt — while the working signal socket keeps serving the existing
//! lifecycle, the owner socket carries the owner-only filesystem mode, and the
//! two contracts stay distinct wire vocabularies. The back-compat case (no meta
//! socket configured) is also covered.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use spirit::schema::meta_signal::{ConfigureRequest, Output as MetaOutput};
use spirit::schema::signal::{Entry, Input, Kind, Magnitude, Output, Query, TopicMatch};
use spirit::{Configuration, Daemon, MetaSignalTransport, SignalTransport};
use tempfile::TempDir;

struct DaemonThread {
    handle: Option<thread::JoinHandle<()>>,
}

impl DaemonThread {
    fn spawn(configuration: Configuration) -> Self {
        let handle = thread::spawn(move || {
            // The daemon serves forever; the test process exits and tears the
            // thread down. A bind error surfaces as a panic in the thread.
            Daemon::new(configuration).run().expect("daemon run");
        });
        Self {
            handle: Some(handle),
        }
    }
}

impl Drop for DaemonThread {
    fn drop(&mut self) {
        // The serve loop never returns on its own; detach the thread so the
        // test process can exit without joining a forever-running daemon.
        drop(self.handle.take());
    }
}

fn wait_for_socket(path: &Path) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("socket did not appear at {}", path.display());
}

fn decision_entry(description: &str) -> Entry {
    Entry {
        topics: vec![String::from("meta-configure")],
        kind: Kind::Decision,
        description: String::from(description),
        magnitude: Magnitude::Maximum,
        privacy: Magnitude::Zero,
    }
}

fn observe_query() -> Query {
    Query {
        topic_match: TopicMatch::full(vec![String::from("meta-configure")]),
        kind: Some(Kind::Decision),
        privacy_selection: spirit::schema::signal::PrivacySelection::default_observation_privacy(),
    }
}

#[test]
fn configure_routes_through_meta_socket_and_moves_archive_target() {
    let temp = TempDir::new().expect("tempdir");
    let working_socket = temp.path().join("spirit.sock");
    let meta_socket = temp.path().join("spirit-meta.sock");
    let initial_database = temp.path().join("initial.sema");
    let reconfigured_database = temp.path().join("reconfigured.sema");

    let configuration = Configuration::new(&working_socket, &initial_database)
        .with_meta_socket_path(&meta_socket);
    let _daemon = DaemonThread::spawn(configuration);
    wait_for_socket(&working_socket);
    wait_for_socket(&meta_socket);

    // CORE PROOF: a Configure request over the META socket is accepted.
    let mut meta_transport =
        MetaSignalTransport::connect(&meta_socket).expect("connect meta socket");
    let new_target = reconfigured_database.to_string_lossy().into_owned();
    let (_route, reply) = meta_transport
        .configure(ConfigureRequest::new(new_target.clone()))
        .expect("exchange configure");
    match reply {
        MetaOutput::Configured(receipt) => {
            assert_eq!(
                receipt.archive_target, new_target,
                "receipt echoes the now-active archive target"
            );
        }
        MetaOutput::Rejected(rejection) => {
            panic!("configure rejected: {:?}", rejection.configure_rejection_reason)
        }
    }

    // EFFECT PROOF: a record written over the WORKING socket after the
    // reconfigure lands at the NEW archive target, not the initial one.
    let mut working_transport =
        SignalTransport::connect(&working_socket).expect("connect working socket");
    let (_output_route, record_output) = working_transport
        .exchange(&Input::Record(decision_entry("intent after reconfigure")))
        .expect("exchange record");
    assert!(
        matches!(record_output, Output::RecordAccepted(_)),
        "working record accepted after reconfigure, got {record_output:?}"
    );

    assert!(
        reconfigured_database.exists(),
        "reconfigured archive target file exists after the working write"
    );

    // The record is observable over the working socket from the new target.
    // Observe stashes its result set, so the reply is a stashed observation
    // whose record_count counts the intent recorded into the reconfigured
    // target — proving the Configure effect actually moved the archive output.
    let mut observe_transport =
        SignalTransport::connect(&working_socket).expect("reconnect working socket");
    let (_observe_route, observed) = observe_transport
        .exchange(&Input::Observe(observe_query()))
        .expect("exchange observe");
    let Output::RecordsStashed(stashed) = observed else {
        panic!("the reconfigured target serves the recorded intent back, got {observed:?}")
    };
    assert_eq!(
        stashed.record_count, 1,
        "the reconfigured target holds exactly the one intent recorded after the reconfigure"
    );
}

#[test]
fn meta_socket_carries_owner_only_mode() {
    let temp = TempDir::new().expect("tempdir");
    let working_socket = temp.path().join("spirit.sock");
    let meta_socket = temp.path().join("spirit-meta.sock");
    let database = temp.path().join("intent.sema");

    let configuration =
        Configuration::new(&working_socket, &database).with_meta_socket_path(&meta_socket);
    let _daemon = DaemonThread::spawn(configuration);
    wait_for_socket(&working_socket);
    wait_for_socket(&meta_socket);

    let meta_mode = std::fs::metadata(&meta_socket)
        .expect("meta socket metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        meta_mode, 0o600,
        "the owner-only meta socket is mode rw------- (the owner gate)"
    );
}

#[test]
fn working_socket_rejects_a_meta_configure_frame() {
    let temp = TempDir::new().expect("tempdir");
    let working_socket = temp.path().join("spirit.sock");
    let meta_socket = temp.path().join("spirit-meta.sock");
    let database = temp.path().join("intent.sema");

    let configuration =
        Configuration::new(&working_socket, &database).with_meta_socket_path(&meta_socket);
    let _daemon = DaemonThread::spawn(configuration);
    wait_for_socket(&working_socket);

    // Send a meta Configure frame to the WORKING socket. The working signal
    // decoder must fail to read it as a signal Output (the two contracts are
    // distinct wire vocabularies): the daemon drops the stream on a decode
    // error, so the reply read returns an error rather than a valid Output.
    let target = database.to_string_lossy().into_owned();
    let mut meta_on_working = MetaSignalTransport::connect(&working_socket)
        .expect("connect meta transport to working socket");
    let result = meta_on_working.configure(ConfigureRequest::new(target));
    assert!(
        result.is_err(),
        "the working socket must not answer a meta Configure as a meta reply"
    );
}

#[test]
fn back_compat_no_meta_socket_serves_working_lifecycle() {
    let temp = TempDir::new().expect("tempdir");
    let working_socket = temp.path().join("spirit.sock");
    let meta_socket = temp.path().join("spirit-meta.sock");
    let database = temp.path().join("intent.sema");

    // No with_meta_socket_path: only the working socket should bind.
    let configuration = Configuration::new(&working_socket, &database);
    let _daemon = DaemonThread::spawn(configuration);
    wait_for_socket(&working_socket);

    assert!(
        !meta_socket.exists(),
        "no meta socket binds when meta_socket_path is None"
    );

    let mut working_transport =
        SignalTransport::connect(&working_socket).expect("connect working socket");
    let (_route, output) = working_transport
        .exchange(&Input::Record(decision_entry("back-compat record")))
        .expect("exchange record");
    assert!(
        matches!(output, Output::RecordAccepted(_)),
        "the single-socket working lifecycle is unchanged, got {output:?}"
    );
}
