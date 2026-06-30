//! Owner-only meta `CollectRemovalCandidates` operation, library level.
//!
//! Proves the locked design: physical deletion is an OWNER-ONLY meta-plane op
//! with NO guardian, mirroring `Import`/`Configure`. The owner configures WHERE
//! the SEPARATE archive database lives (meta `Configure`), then the owner issues
//! the meta `CollectRemovalCandidates`. The operation archives the matching
//! records into the separate archive database at the configured target, removes
//! them from the live log, and replies meta
//! `RemovalCandidatesCollected { removal_candidates_collection { removal_archive_records,
//! removed_identifiers, skipped_candidates } }`. Non-matching records are left in
//! the live log; the archive database is a distinct `*.sema` file from the live
//! intent log. There is no working-socket physical-deletion path.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use spirit::schema::meta_signal::{
    ArchiveDatabaseTarget, CollectRemovalCandidatesRequest, ConfigureRequest, Output as MetaOutput,
};
use spirit::schema::signal::{
    CertaintySelection, Description, DomainMatch, DomainScopes, Domains, Entry,
    ImportanceSelection, Input, Justification, Kind, Magnitude, Output, Privacy, PrivacySelection,
    Query, QuoteText, Reasoning, RecordRequest, RemovalCandidateCollection, SelectedKind,
    Testimony, VerbatimQuote,
};
use spirit::{Configuration, Daemon, Engine, MetaSignalTransport, SignalTransport, Store};
use tempfile::TempDir;

fn domains(label: &str) -> Domains {
    Domains::from_strings(vec![String::from(label)])
}

fn configure_request(archive_database_target: ArchiveDatabaseTarget) -> ConfigureRequest {
    ConfigureRequest::new(archive_database_target, None, None, None)
}

fn domain_scopes(label: &str) -> DomainScopes {
    DomainScopes::from_strings(vec![String::from(label)])
}

fn entry(domain: &str, description: &str) -> Entry {
    entry_with_certainty(domain, description, Magnitude::Maximum)
}

fn entry_with_certainty(domain: &str, description: &str, magnitude: Magnitude) -> Entry {
    Entry {
        domains: domains(domain),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: magnitude.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: spirit::schema::signal::Referents::new(vec![
            spirit::schema::signal::Referent::new("spirit"),
        ]),
    }
}

fn record_request(entry: Entry) -> RecordRequest {
    let statement = entry.description.payload().clone();
    RecordRequest {
        entry,
        justification: Justification {
            testimony: Testimony::new(vec![VerbatimQuote::new(
                QuoteText::new(statement.clone()),
                None,
            )]),
            reasoning: Reasoning::new(statement),
        },
    }
}

fn domain_query(domain: &str) -> Query {
    Query {
        domain_match: DomainMatch::full(domain_scopes(domain)),
        keyword_match: spirit::schema::signal::KeywordMatch::Any,
        text_match: spirit::schema::signal::TextMatch::Any,
        referent_selection: spirit::schema::signal::ReferentSelection::Any,
        selected_kind: SelectedKind::new(Some(Kind::Decision)),
        privacy_selection: PrivacySelection::default_observation_privacy(),
        certainty_selection: CertaintySelection::default_observation_certainty(),
        importance_selection: ImportanceSelection::default_observation_importance(),
    }
}

fn removal_candidate_query(domain: &str) -> Query {
    Query {
        domain_match: DomainMatch::full(domain_scopes(domain)),
        keyword_match: spirit::schema::signal::KeywordMatch::Any,
        text_match: spirit::schema::signal::TextMatch::Any,
        referent_selection: spirit::schema::signal::ReferentSelection::Any,
        selected_kind: SelectedKind::new(Some(Kind::Decision)),
        privacy_selection: PrivacySelection::default_observation_privacy(),
        certainty_selection: CertaintySelection::removal_candidate_certainty(),
        importance_selection: ImportanceSelection::default_observation_importance(),
    }
}

fn removal_candidate_collection(domain: &str) -> RemovalCandidateCollection {
    let statement = format!("collect {domain} removal candidates");
    RemovalCandidateCollection {
        record_query: removal_candidate_query(domain).into(),
        justification: Justification {
            testimony: Testimony::new(vec![VerbatimQuote::new(
                QuoteText::new(statement.clone()),
                None,
            )]),
            reasoning: Reasoning::new(statement),
        },
    }
}

fn record(engine: &mut Engine, entry: Entry) {
    let output = engine
        .handle(Input::record(record_request(entry)))
        .into_root();
    assert!(
        matches!(output, Output::RecordAccepted(_)),
        "record accepted, got {output:?}"
    );
}

#[test]
fn collect_removal_candidates_archives_to_separate_db_and_removes_from_live() {
    let temp = TempDir::new().expect("tempdir");
    let live_database = temp.path().join("live.sema");
    let archive_database = temp.path().join("archive.sema");

    let mut engine = Engine::new(Store::open(&live_database).expect("open live store"));
    engine.start().expect("engine start");

    // OWNER configures WHERE the separate archive database lives.
    let archive_target =
        ArchiveDatabaseTarget::path(archive_database.to_string_lossy().into_owned().into());
    let configure = engine.configure(configure_request(archive_target));
    assert!(
        matches!(
            configure,
            spirit::schema::meta_signal::Output::Configured(_)
        ),
        "owner configure accepted, got {configure:?}"
    );

    // Two live records: one is a removal candidate (`Governing`), one stays
    // (`Meaning`).
    record(
        &mut engine,
        entry_with_certainty("governing", "obsolete intent to retire", Magnitude::Zero),
    );
    record(
        &mut engine,
        entry("meaning", "intent that must remain live"),
    );

    // OWNER collects the removal candidates matching the `Governing` query via
    // the owner-only meta plane — there is no working-socket deletion path.
    let collection = removal_candidate_collection("governing");
    let reply = engine.collect_removal_candidates(CollectRemovalCandidatesRequest::new(collection));
    let MetaOutput::RemovalCandidatesCollected(collected) = reply else {
        panic!("expected RemovalCandidatesCollected, got {reply:?}")
    };
    let collected = &collected.payload().removal_candidates_collection;

    // CORRECT REPLY: one archived record, one removed identifier, no skips.
    assert_eq!(
        collected.removal_archive_records.payload().len(),
        1,
        "exactly the one stale record was archived"
    );
    assert_eq!(
        collected.removed_identifiers.payload().len(),
        1,
        "exactly the one stale record was removed from the live log"
    );
    assert!(
        collected.skipped_removal_candidates.payload().is_empty(),
        "no candidate was skipped"
    );
    assert_eq!(
        collected.removal_archive_records.payload()[0]
            .entry
            .description
            .payload(),
        "obsolete intent to retire",
        "the archived record is the stale one"
    );
    assert_eq!(
        collected.removal_archive_records.payload()[0].record_identifier,
        *collected.removed_identifiers.payload()[0].payload(),
        "the archived record identifier matches the removed live identifier"
    );

    // REMOVED FROM LIVE: the stale record is gone, the keep record remains.
    let stale_observe = engine
        .handle(Input::observe(domain_query("governing")))
        .into_root();
    assert!(
        matches!(stale_observe, Output::Error(_)),
        "the stale record is gone from the live log (no matching record), got {stale_observe:?}"
    );
    let keep_observe = engine
        .handle(Input::observe(domain_query("meaning")))
        .into_root();
    let Output::RecordsStashed(kept) = keep_observe else {
        panic!("the keep record still serves from the live log, got {keep_observe:?}")
    };
    assert_eq!(
        *kept.record_count.payload(),
        1,
        "the keep record stayed in the live log"
    );

    // ARCHIVE-TO-SEPARATE-DB: the archive database is a distinct file holding
    // the archived record. Reopening it as a store shows exactly one record.
    assert!(
        archive_database.exists(),
        "the separate archive database file was created at the configured target"
    );
    assert_ne!(
        archive_database, live_database,
        "the archive database is a separate file from the live intent log"
    );
    let archive_store = Store::open(&archive_database).expect("reopen archive store");
    assert_eq!(
        archive_store.len(),
        1,
        "the separate archive database holds exactly the one archived record"
    );

    engine.stop().expect("engine stop");
}

#[test]
fn collect_removal_candidates_with_no_matches_archives_nothing() {
    let temp = TempDir::new().expect("tempdir");
    let live_database = temp.path().join("live.sema");
    let archive_database = temp.path().join("archive.sema");

    let mut engine = Engine::new(Store::open(&live_database).expect("open live store"));
    engine.start().expect("engine start");
    let archive_target =
        ArchiveDatabaseTarget::path(archive_database.to_string_lossy().into_owned().into());
    engine.configure(configure_request(archive_target));

    record(
        &mut engine,
        entry("meaning", "intent that must remain live"),
    );

    let collection = removal_candidate_collection("governing");
    let reply = engine.collect_removal_candidates(CollectRemovalCandidatesRequest::new(collection));
    let MetaOutput::RemovalCandidatesCollected(collected) = reply else {
        panic!("expected RemovalCandidatesCollected, got {reply:?}")
    };
    let collected = &collected.payload().removal_candidates_collection;
    assert!(
        collected.removal_archive_records.payload().is_empty(),
        "nothing matched, so nothing was archived"
    );
    assert!(
        collected.removed_identifiers.payload().is_empty(),
        "nothing matched, so nothing was removed"
    );

    // The keep record is untouched in the live log.
    let keep_observe = engine
        .handle(Input::observe(domain_query("meaning")))
        .into_root();
    let Output::RecordsStashed(kept) = keep_observe else {
        panic!("the keep record still serves from the live log, got {keep_observe:?}")
    };
    assert_eq!(
        *kept.record_count.payload(),
        1,
        "the keep record stayed in the live log"
    );

    engine.stop().expect("engine stop");
}

#[test]
fn collect_removal_candidates_requires_zero_certainty() {
    let temp = TempDir::new().expect("tempdir");
    let live_database = temp.path().join("live.sema");
    let archive_database = temp.path().join("archive.sema");

    let mut engine = Engine::new(Store::open(&live_database).expect("open live store"));
    engine.start().expect("engine start");
    let archive_target =
        ArchiveDatabaseTarget::path(archive_database.to_string_lossy().into_owned().into());
    engine.configure(configure_request(archive_target));

    record(
        &mut engine,
        entry("governing", "same domain but still live"),
    );

    let collection = removal_candidate_collection("governing");
    let reply = engine.collect_removal_candidates(CollectRemovalCandidatesRequest::new(collection));
    let MetaOutput::RemovalCandidatesCollected(collected) = reply else {
        panic!("expected RemovalCandidatesCollected, got {reply:?}")
    };
    let collected = &collected.payload().removal_candidates_collection;
    assert!(
        collected.removal_archive_records.payload().is_empty(),
        "nonzero certainty is not a removal candidate"
    );
    assert!(
        collected.removed_identifiers.payload().is_empty(),
        "nonzero certainty remains live"
    );
    assert!(
        collected.skipped_removal_candidates.payload().is_empty(),
        "non-candidates are filtered out before the operational skip phase"
    );

    let live_observe = engine
        .handle(Input::observe(domain_query("governing")))
        .into_root();
    let Output::RecordsStashed(live) = live_observe else {
        panic!("the nonzero record still serves from the live log, got {live_observe:?}")
    };
    assert_eq!(
        *live.record_count.payload(),
        1,
        "the nonzero record stayed live"
    );

    engine.stop().expect("engine stop");
}

// ---------------------------------------------------------------------------
// Boundary — the same proof, but driven as a real frame through the daemon's
// owner-only meta socket (`handle_meta_connection`'s `CollectRemovalCandidates`
// arm). `CollectRemovalCandidates` is now the ONLY physical-deletion path, so
// the meta-socket arm earns an end-to-end witness mirroring the `Import`
// boundary test. The daemon runs in-process on its own thread, binding real
// Unix sockets in an isolated tempdir store.
// ---------------------------------------------------------------------------

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

fn record_over_socket(working_socket: &Path, entry: Entry) {
    let mut transport = SignalTransport::connect(working_socket).expect("connect working socket");
    let (_route, output) = transport
        .exchange(&Input::record(record_request(entry)))
        .expect("exchange record");
    assert!(
        matches!(output, Output::RecordAccepted(_)),
        "record accepted over the working socket, got {output:?}"
    );
}

#[test]
fn collect_removal_candidates_archives_and_removes_over_the_meta_socket() {
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

    // OWNER configures WHERE the separate archive database lives, over the meta
    // socket — a real Configure frame through `handle_meta_connection`.
    let archive_target =
        ArchiveDatabaseTarget::path(archive_database.to_string_lossy().into_owned().into());
    let mut meta_transport =
        MetaSignalTransport::connect(&meta_socket).expect("connect meta socket");
    let (_configure_route, configure_reply) = meta_transport
        .configure(configure_request(archive_target).into())
        .expect("exchange configure");
    assert!(
        matches!(configure_reply, MetaOutput::Configured(_)),
        "owner configure accepted over the meta socket, got {configure_reply:?}"
    );

    // Two live records over the WORKING socket: one zero-certainty removal
    // candidate (`governing`), one that must remain (`meaning`).
    record_over_socket(
        &working_socket,
        entry_with_certainty("governing", "obsolete intent to retire", Magnitude::Zero),
    );
    record_over_socket(
        &working_socket,
        entry("meaning", "intent that must remain live"),
    );

    // OWNER collects the removal candidates over the meta socket — the single
    // physical-deletion path, driven as a real frame through the daemon arm. The
    // meta connection serves one request per connection (`handle_meta_connection`
    // reads one frame, replies, and closes), so reconnect for this exchange.
    let collection = removal_candidate_collection("governing");
    let mut collect_transport =
        MetaSignalTransport::connect(&meta_socket).expect("reconnect meta socket");
    let (_collect_route, collect_reply) = collect_transport
        .collect_removal_candidates(CollectRemovalCandidatesRequest::new(collection).into())
        .expect("exchange collect removal candidates");
    let MetaOutput::RemovalCandidatesCollected(collected) = collect_reply else {
        panic!("expected RemovalCandidatesCollected over the socket, got {collect_reply:?}")
    };
    let collected = &collected.payload().removal_candidates_collection;

    // CORRECT REPLY OVER THE WIRE: one archived record, one removed identifier,
    // no skips.
    assert_eq!(
        collected.removal_archive_records.payload().len(),
        1,
        "exactly the one stale record was archived over the socket"
    );
    assert_eq!(
        collected.removed_identifiers.payload().len(),
        1,
        "exactly the one stale record was removed from the live log over the socket"
    );
    assert!(
        collected.skipped_removal_candidates.payload().is_empty(),
        "no candidate was skipped"
    );
    assert_eq!(
        collected.removal_archive_records.payload()[0]
            .entry
            .description
            .payload(),
        "obsolete intent to retire",
        "the archived record is the stale one"
    );

    // REMOVED FROM LIVE: the stale record is gone, the keep record remains —
    // confirmed by observe over the working socket.
    let mut stale_transport =
        SignalTransport::connect(&working_socket).expect("connect working socket");
    let (_stale_route, stale_observe) = stale_transport
        .exchange(&Input::observe(domain_query("governing")))
        .expect("exchange stale observe");
    assert!(
        matches!(stale_observe, Output::Error(_)),
        "the stale record is gone from the live log over the socket, got {stale_observe:?}"
    );
    let mut keep_transport =
        SignalTransport::connect(&working_socket).expect("reconnect working socket");
    let (_keep_route, keep_observe) = keep_transport
        .exchange(&Input::observe(domain_query("meaning")))
        .expect("exchange keep observe");
    let Output::RecordsStashed(kept) = keep_observe else {
        panic!("the keep record still serves from the live log, got {keep_observe:?}")
    };
    assert_eq!(
        *kept.record_count.payload(),
        1,
        "the keep record stayed in the live log"
    );

    // ARCHIVE-TO-SEPARATE-DB: the archive database is a distinct file holding the
    // one archived record.
    assert!(
        archive_database.exists(),
        "the separate archive database file was created over the socket"
    );
    assert_ne!(
        archive_database, live_database,
        "the archive database is a separate file from the live intent log"
    );
    let archive_store = Store::open(&archive_database).expect("reopen archive store");
    assert_eq!(
        archive_store.len(),
        1,
        "the separate archive database holds exactly the one archived record"
    );
}
