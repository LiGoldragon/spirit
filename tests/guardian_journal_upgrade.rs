//! Graceful in-place upgrade of the guardian audit journal across a
//! storage-schema bump.
//!
//! The journal persists rkyv enums whose positional discriminants can shift
//! across schema arcs. A current daemon must NOT decode an older journal's
//! bytes under the new layout.
//!
//! The guard rides the FILENAME: the journal lives at
//! `<live-stem>.guardian.v{N}.sema`, so a schema bump lands a FRESH current
//! file and leaves the old file orphaned and untouched — never opened, never
//! decoded. Were the filename suffix NOT bumped alongside the schema version,
//! the daemon would reopen the existing old file under the current expected
//! version and hard-fail on the kernel's `SchemaVersionMismatch` guard. This
//! test reconstructs a real previous journal and witnesses the graceful orphan.

#![cfg(feature = "agent-guardian")]

use sema_engine::{
    Assertion, Engine as SemaDatabase, EngineOpen, EngineRecord, FamilyName, RecordKey, SchemaHash,
    SchemaVersion, TableDescriptor, TableName,
};
use spirit::Store;
use tempfile::TempDir;

// The previous journal's exact storage coordinates, as the previous daemon
// wrote them.
const PREVIOUS_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(5);
const CURRENT_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(6);
const GUARDIAN_DECISIONS_TABLE: TableName = TableName::new("guardian-decisions");
const PREVIOUS_FAMILY_LABEL: &str = "spirit:guardian-journal:v5";

/// A stand-in for the crate-private previous `GuardianJournalEntry`. The
/// current daemon never decodes this record — it opens a different file — so
/// only the table coordinates (name, family, schema version) need to match the
/// real previous journal, not the record's field shape.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct LegacyGuardianJournalEntry {
    decision_identifier: String,
}

impl EngineRecord for LegacyGuardianJournalEntry {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.decision_identifier.clone())
    }
}

#[test]
fn current_daemon_starts_clean_against_an_existing_previous_journal() {
    let temp = TempDir::new().expect("tempdir");
    let live_database = temp.path().join("intent.sema");
    let previous_journal = temp.path().join(format!(
        "intent.guardian.v{}.sema",
        PREVIOUS_SCHEMA_VERSION.value()
    ));
    let current_journal = temp.path().join(format!(
        "intent.guardian.v{}.sema",
        CURRENT_SCHEMA_VERSION.value()
    ));

    // Write a real previous journal beside the live database: the exact table
    // name, family label, and schema version the previous daemon used,
    // carrying one entry.
    {
        let mut database =
            SemaDatabase::open(EngineOpen::new(&previous_journal, PREVIOUS_SCHEMA_VERSION))
                .expect("open previous journal");
        let decisions = database
            .register_table(TableDescriptor::new(
                GUARDIAN_DECISIONS_TABLE,
                FamilyName::new("GuardianDecisionsFamily"),
                SchemaHash::for_label(PREVIOUS_FAMILY_LABEL),
            ))
            .expect("register previous family");
        database
            .assert(Assertion::new(
                decisions,
                LegacyGuardianJournalEntry {
                    decision_identifier: String::from("guardian-decision-1"),
                },
            ))
            .expect("append previous entry");
    }
    assert!(
        previous_journal.exists(),
        "the previous journal exists on disk before the upgrade"
    );
    assert!(
        !current_journal.exists(),
        "no current journal exists yet — the live daemon only wrote the previous version"
    );

    // The current daemon opens the store and reads its guardian journal. It
    // must start cleanly and see ZERO decisions: the current journal is a
    // fresh file at the current path, and the stale previous entry is never
    // read.
    let store = Store::open(&live_database)
        .expect("current store opens against the existing previous journal");
    assert_eq!(
        store
            .guardian_decision_count()
            .expect("current journal opens fresh and reads cleanly"),
        0,
        "the current daemon opens a fresh journal and ignores the stale previous entries"
    );

    // The old journal stays on disk, orphaned and untouched; the current daemon
    // created its own fresh journal at the distinct current path.
    assert!(
        previous_journal.exists(),
        "the previous journal is left orphaned on disk, neither deleted nor rewritten"
    );
    assert!(
        current_journal.exists(),
        "the current daemon created its own fresh journal at the distinct current path"
    );
}
