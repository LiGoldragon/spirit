//! Owner-only meta `Configure` route end-to-end.
//!
//! Proves the meta-signal listener wiring: a `Configure` request routes through
//! the owner-only meta socket, applies the owner-config effect (stores WHERE the
//! SEPARATE archive database lives), and replies with a typed receipt — WITHOUT
//! touching the live intent-log database. The working signal socket keeps
//! serving the existing lifecycle, the owner socket carries the owner-only
//! filesystem mode, and the two contracts stay distinct wire vocabularies.
//! A daemon configuration without the required meta socket is rejected before
//! serving.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use spirit::schema::meta_signal::{ArchiveDatabaseTarget, ConfigureRequest, Output as MetaOutput};
use spirit::schema::signal::{
    Description, DomainMatch, DomainScopes, Domains, Entry, ImportanceSelection, Input,
    Justification, Kind, Magnitude, Output, Privacy, Query, RecordRequest, StatementText,
};
use spirit::{
    Configuration, Daemon, DaemonError, MetaSignalTransport, SignalTransport, SpiritDaemon,
};
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
        domains: Domains::from_strings(vec![String::from("meta-configure")]),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: Magnitude::Maximum.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: spirit::schema::signal::Referents::new(Vec::new()),
    }
}

fn record_request(description: &str) -> RecordRequest {
    RecordRequest {
        entry: decision_entry(description),
        justification: Justification {
            statement_text: StatementText::new(description),
            context: None,
        },
    }
}

fn observe_query() -> Query {
    Query {
        domain_match: DomainMatch::full(DomainScopes::from_strings(vec![String::from(
            "meta-configure",
        )])),
        keyword_match: spirit::schema::signal::KeywordMatch::Any,
        text_match: spirit::schema::signal::TextMatch::Any,
        referent_selection: spirit::schema::signal::ReferentSelection::Any,
        kind: Some(Kind::Decision),
        privacy_selection: spirit::schema::signal::PrivacySelection::default_observation_privacy(),
        certainty_selection:
            spirit::schema::signal::CertaintySelection::default_observation_certainty(),
        importance_selection: ImportanceSelection::default_observation_importance(),
    }
}

#[test]
fn configure_sets_archive_target_and_leaves_live_database_unchanged() {
    let temp = TempDir::new().expect("tempdir");
    let working_socket = temp.path().join("spirit.sock");
    let meta_socket = temp.path().join("spirit-meta.sock");
    let live_database = temp.path().join("live.sema");
    let archive_database = temp.path().join("archive.sema");

    let configuration =
        Configuration::new(&working_socket, &live_database).with_meta_socket_path(&meta_socket);
    let _daemon = DaemonThread::spawn(configuration);
    wait_for_socket(&working_socket);
    wait_for_socket(&meta_socket);

    // CORE PROOF (1): a Configure request over the META socket is accepted and
    // the receipt echoes the now-active archive target. Configure sets WHERE
    // the SEPARATE archive database lives — it is a typed `ArchiveDatabaseTarget`,
    // not a string, and the live database path is never named in it.
    let archive_target =
        ArchiveDatabaseTarget::path(archive_database.to_string_lossy().into_owned().into());
    let mut meta_transport =
        MetaSignalTransport::connect(&meta_socket).expect("connect meta socket");
    let (_route, reply) = meta_transport
        .configure(ConfigureRequest::new(archive_target.clone()).into())
        .expect("exchange configure");
    match reply {
        MetaOutput::Configured(receipt) => {
            assert_eq!(
                receipt.payload().archive_database_target,
                archive_target,
                "receipt echoes the now-active archive target"
            );
        }
        MetaOutput::Rejected(rejection) => {
            panic!(
                "configure rejected: {:?}",
                rejection.payload().configure_rejection_reason
            )
        }
    }

    // CORE PROOF (2): the LIVE database is UNCHANGED by Configure. A record
    // written over the WORKING socket AFTER the Configure lands in the same live
    // database the daemon opened — Configure never re-pointed, moved, or touched
    // the live log.
    let mut working_transport =
        SignalTransport::connect(&working_socket).expect("connect working socket");
    let (_output_route, record_output) = working_transport
        .exchange(&Input::record(record_request("intent after configure")))
        .expect("exchange record");
    assert!(
        matches!(record_output, Output::RecordAccepted(_)),
        "working record accepted after configure, got {record_output:?}"
    );

    assert!(
        live_database.exists(),
        "the live database is the one that received the working write"
    );
    assert!(
        !archive_database.exists(),
        "Configure must NOT create or open the archive database — it only stored the target"
    );

    // CORE PROOF (3): the record sent after Configure is still found in the LIVE
    // database. Observe stashes its result set, so the reply is a stashed
    // observation whose record_count counts the intent recorded into the live
    // log — proving the live database is intact and serving working reads.
    let mut observe_transport =
        SignalTransport::connect(&working_socket).expect("reconnect working socket");
    let (_observe_route, observed) = observe_transport
        .exchange(&Input::observe(observe_query()))
        .expect("exchange observe");
    let Output::RecordsStashed(stashed) = observed else {
        panic!("the live database serves the recorded intent back, got {observed:?}")
    };
    assert_eq!(
        stashed.record_count, 1,
        "the live database holds exactly the one intent recorded after the Configure"
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
    let target = ArchiveDatabaseTarget::path(database.to_string_lossy().into_owned().into());
    let mut meta_on_working = MetaSignalTransport::connect(&working_socket)
        .expect("connect meta transport to working socket");
    let result = meta_on_working.configure(ConfigureRequest::new(target).into());
    assert!(
        result.is_err(),
        "the working socket must not answer a meta Configure as a meta reply"
    );
}

#[test]
fn daemon_rejects_missing_meta_socket_before_serving() {
    let temp = TempDir::new().expect("tempdir");
    let working_socket = temp.path().join("spirit.sock");
    let database = temp.path().join("intent.sema");

    let configuration = Configuration::new(&working_socket, &database);

    assert!(
        matches!(
            Daemon::new(configuration).run(),
            Err(DaemonError::<SpiritDaemon>::MissingMetaSocket)
        ),
        "a daemon without the meta slot must fail before serving"
    );
}
