//! Peer-callable `CollectRemovalCandidates` working operation, library level.
//!
//! Proves the locked design's authority split: the OWNER configures WHERE the
//! SEPARATE archive database lives (meta `Configure`), and a PEER does the
//! archiving (working `CollectRemovalCandidates`). The operation archives the
//! matching records into the separate archive database at the configured
//! target, removes them from the live log, and replies
//! `RemovalCandidatesCollected { archived_records, removed_identifiers,
//! skipped_candidates }`. Non-matching records are left in the live log; the
//! archive database is a distinct `*.sema` file from the live intent log.

use spirit::schema::meta_signal::{ArchiveDatabaseTarget, ConfigureRequest};
use spirit::schema::signal::{
    Entry, Input, Kind, Magnitude, Output, PrivacySelection, Query, RemovalCandidateCollection,
    TopicMatch,
};
use spirit::{Engine, Store};
use tempfile::TempDir;

fn entry(topic: &str, description: &str) -> Entry {
    Entry {
        topics: vec![String::from(topic)],
        kind: Kind::Decision,
        description: String::from(description),
        magnitude: Magnitude::Maximum,
        privacy: Magnitude::Zero,
    }
}

fn topic_query(topic: &str) -> Query {
    Query {
        topic_match: TopicMatch::full(vec![String::from(topic)]),
        kind: Some(Kind::Decision),
        privacy_selection: PrivacySelection::default_observation_privacy(),
    }
}

fn record(engine: &mut Engine, entry: Entry) {
    let output = engine.handle(Input::Record(entry)).into_root();
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
        ArchiveDatabaseTarget::path(archive_database.to_string_lossy().into_owned());
    let configure = engine.configure(ConfigureRequest::new(archive_target));
    assert!(
        matches!(
            configure,
            spirit::schema::meta_signal::Output::Configured(_)
        ),
        "owner configure accepted, got {configure:?}"
    );

    // Two live records: one is a removal candidate (topic `stale`), one stays
    // (topic `keep`).
    record(&mut engine, entry("stale", "obsolete intent to retire"));
    record(&mut engine, entry("keep", "intent that must remain live"));

    // PEER collects the removal candidates matching the `stale` query.
    let collection = RemovalCandidateCollection::new(topic_query("stale"));
    let reply = engine
        .handle(Input::CollectRemovalCandidates(collection))
        .into_root();
    let Output::RemovalCandidatesCollected(collected) = reply else {
        panic!("expected RemovalCandidatesCollected, got {reply:?}")
    };

    // CORRECT REPLY: one archived record, one removed identifier, no skips.
    assert_eq!(
        collected.archived_records.len(),
        1,
        "exactly the one stale record was archived"
    );
    assert_eq!(
        collected.removed_identifiers.len(),
        1,
        "exactly the one stale record was removed from the live log"
    );
    assert!(
        collected.skipped_removal_candidates.is_empty(),
        "no candidate was skipped"
    );
    assert_eq!(
        collected.archived_records[0].entry.description, "obsolete intent to retire",
        "the archived record is the stale one"
    );
    assert_eq!(
        collected.archived_records[0].record_identifier, collected.removed_identifiers[0],
        "the archived record identifier matches the removed live identifier"
    );

    // REMOVED FROM LIVE: the stale record is gone, the keep record remains.
    let stale_observe = engine
        .handle(Input::Observe(topic_query("stale")))
        .into_root();
    assert!(
        matches!(stale_observe, Output::Error(_)),
        "the stale record is gone from the live log (no matching record), got {stale_observe:?}"
    );
    let keep_observe = engine
        .handle(Input::Observe(topic_query("keep")))
        .into_root();
    let Output::RecordsStashed(kept) = keep_observe else {
        panic!("the keep record still serves from the live log, got {keep_observe:?}")
    };
    assert_eq!(
        kept.record_count, 1,
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
        ArchiveDatabaseTarget::path(archive_database.to_string_lossy().into_owned());
    engine.configure(ConfigureRequest::new(archive_target));

    record(&mut engine, entry("keep", "intent that must remain live"));

    let collection = RemovalCandidateCollection::new(topic_query("stale"));
    let reply = engine
        .handle(Input::CollectRemovalCandidates(collection))
        .into_root();
    let Output::RemovalCandidatesCollected(collected) = reply else {
        panic!("expected RemovalCandidatesCollected, got {reply:?}")
    };
    assert!(
        collected.archived_records.is_empty(),
        "nothing matched, so nothing was archived"
    );
    assert!(
        collected.removed_identifiers.is_empty(),
        "nothing matched, so nothing was removed"
    );

    // The keep record is untouched in the live log.
    let keep_observe = engine
        .handle(Input::Observe(topic_query("keep")))
        .into_root();
    let Output::RecordsStashed(kept) = keep_observe else {
        panic!("the keep record still serves from the live log, got {keep_observe:?}")
    };
    assert_eq!(
        kept.record_count, 1,
        "the keep record stayed in the live log"
    );

    engine.stop().expect("engine stop");
}
