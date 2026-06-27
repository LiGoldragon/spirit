//! Graceful in-place upgrade of the guardian audit journal across the
//! storage-schema bump (v4 -> v5).
//!
//! The journal persists rkyv enums whose positional discriminants shifted in
//! this arc: `GuardianRejectionReason` gained `Matter` mid-enum and
//! `GuardianOperation` dropped its removal arms. A v5-labelled daemon must NOT
//! decode the live daemon's existing v4 journal bytes under the new layout.
//!
//! The guard rides the FILENAME: the journal lives at
//! `<live-stem>.guardian.v{N}.sema`, so a schema bump lands a FRESH v5 file and
//! leaves the old v4 file orphaned and untouched — never opened, never decoded.
//! Were the filename suffix NOT bumped alongside the schema version, the v5
//! daemon would reopen the existing v4-stamped file with an expected version of
//! 5 and hard-fail on the kernel's `SchemaVersionMismatch` guard. This test
//! reconstructs a real v4 journal and witnesses the graceful orphan.

#![cfg(feature = "agent-guardian")]

use sema_engine::{
    Assertion, Engine as SemaDatabase, EngineOpen, EngineRecord, FamilyName, RecordKey, SchemaHash,
    SchemaVersion, TableDescriptor, TableName,
};
use spirit::Store;
use tempfile::TempDir;

// The v4 journal's exact storage coordinates, as the v4 daemon wrote them.
const V4_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(4);
const GUARDIAN_DECISIONS_TABLE: TableName = TableName::new("guardian-decisions");
const V4_FAMILY_LABEL: &str = "spirit:guardian-journal:v4";

/// A stand-in for the crate-private v4 `GuardianJournalEntry`. The v5 daemon
/// never decodes this record — it opens a different file — so only the table
/// coordinates (name, family, schema version) need to match the real v4
/// journal, not the record's field shape.
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
fn v5_daemon_starts_clean_against_an_existing_v4_journal() {
    let temp = TempDir::new().expect("tempdir");
    let live_database = temp.path().join("intent.sema");
    let v4_journal = temp.path().join("intent.guardian.v4.sema");
    let v5_journal = temp.path().join("intent.guardian.v5.sema");

    // Write a real v4 journal beside the live database: the exact table name,
    // family label, and schema version the v4 daemon used, carrying one entry.
    {
        let mut database = SemaDatabase::open(EngineOpen::new(&v4_journal, V4_SCHEMA_VERSION))
            .expect("open v4 journal");
        let decisions = database
            .register_table(TableDescriptor::new(
                GUARDIAN_DECISIONS_TABLE,
                FamilyName::new("GuardianDecisionsFamily"),
                SchemaHash::for_label(V4_FAMILY_LABEL),
            ))
            .expect("register v4 family");
        database
            .assert(Assertion::new(
                decisions,
                LegacyGuardianJournalEntry {
                    decision_identifier: String::from("guardian-decision-1"),
                },
            ))
            .expect("append v4 entry");
    }
    assert!(
        v4_journal.exists(),
        "the v4 journal exists on disk before the upgrade"
    );
    assert!(
        !v5_journal.exists(),
        "no v5 journal exists yet — the live daemon only wrote v4"
    );

    // The v5 daemon opens the store and reads its guardian journal. It must
    // start cleanly and see ZERO decisions: the v5 journal is a fresh file at
    // the v5 path, and the stale v4 entry is never read. (If the filename
    // suffix were not bumped, this call would reopen the v4 file under an
    // expected version of 5 and fail with `SchemaVersionMismatch`.)
    let store =
        Store::open(&live_database).expect("v5 store opens against the existing v4 journal");
    assert_eq!(
        store
            .guardian_decision_count()
            .expect("v5 journal opens fresh and reads cleanly"),
        0,
        "the v5 daemon opens a fresh journal and ignores the stale v4 entries"
    );

    // The old v4 journal stays on disk, orphaned and untouched; the v5 daemon
    // created its own fresh journal at the distinct v5 path.
    assert!(
        v4_journal.exists(),
        "the v4 journal is left orphaned on disk, neither deleted nor rewritten"
    );
    assert!(
        v5_journal.exists(),
        "the v5 daemon created its own fresh journal at the distinct v5 path"
    );
}
