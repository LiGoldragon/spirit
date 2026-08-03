use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use sema_engine::{
    Assertion, Engine as SemaDatabase, EngineOpen, QueryPlan, SchemaVersion, VersionedStoreName,
    VersioningPolicy,
};
use spirit::{
    Store, StoreMigration, StoreMigrationOutput, StoreMigrationRequest,
    production_migration::v13,
    schema::{sema::RecordFamily, signal::Magnitude},
};
use tempfile::TempDir;

fn legacy_entry(
    description: &str,
    certainty: v13::Magnitude,
    privacy: v13::Magnitude,
    referents: &[&str],
    importance: v13::Magnitude,
) -> v13::Entry {
    v13::Entry {
        domains: v13::Domains::new(vec![v13::Domain::All]),
        kind: v13::Kind::Decision,
        description: v13::Description::new(description.to_owned()),
        certainty: v13::Certainty::new(certainty),
        importance: v13::Importance::new(importance),
        privacy: v13::Privacy::new(privacy),
        referents: v13::Referents::new(
            referents
                .iter()
                .map(|referent| v13::Referent::new((*referent).to_owned()))
                .collect(),
        ),
    }
}

fn legacy_record(identifier: &str, entry: v13::Entry) -> v13::StoredRecord {
    v13::StoredRecord {
        record_identifier: v13::RecordIdentifier::new(identifier.to_owned()),
        entry,
    }
}

fn seed_v13_live(path: &Path) {
    let layout = v13::FrozenLayout::version_thirteen();
    let mut database =
        SemaDatabase::open(EngineOpen::new(path, v13::SCHEMA_VERSION).with_versioning(
            VersioningPolicy::new(VersionedStoreName::new(v13::STORE_NAME)),
        ))
        .expect("open v13 live fixture");
    let records = database
        .register_table(layout.records_descriptor())
        .expect("register v13 records");
    let referents = database
        .register_table(layout.referents_descriptor())
        .expect("register v13 referents");
    let migrations = database
        .register_table(layout.migrations_descriptor())
        .expect("register v13 migrations");

    database
        .assert(Assertion::new(
            records,
            legacy_record(
                "zero-certainty",
                legacy_entry(
                    "zero certainty survives as an ordinary record",
                    v13::Magnitude::Zero,
                    v13::Magnitude::Zero,
                    &["retired-zero-topic"],
                    v13::Magnitude::Low,
                ),
            ),
        ))
        .expect("assert zero-certainty row");
    database
        .assert(Assertion::new(
            records,
            legacy_record(
                "formerly-private",
                legacy_entry(
                    "formerly private survives as an ordinary record",
                    v13::Magnitude::VeryHigh,
                    v13::Magnitude::Maximum,
                    &["retired-private-topic"],
                    v13::Magnitude::High,
                ),
            ),
        ))
        .expect("assert private row");
    database
        .assert(Assertion::new(
            referents,
            v13::StoredReferent {
                referent: v13::Referent::new("retired-private-topic".to_owned()),
                aliases: v13::Aliases::new(v13::Referents::new(vec![v13::Referent::new(
                    "unique-retired-alias".to_owned(),
                )])),
            },
        ))
        .expect("assert v13 referent catalogue row");
    database
        .assert(Assertion::new(
            migrations,
            v13::Migration {
                source_schema_version: v13::SourceSchemaVersion::new(7),
                migrated_record_count: v13::MigratedRecordCount::new(99),
                migrated_referent_count: v13::MigratedReferentCount::new(42),
            },
        ))
        .expect("assert obsolete migration row");
}

fn seed_v13_archive(path: &Path) {
    let layout = v13::FrozenLayout::version_thirteen();
    let mut database = SemaDatabase::open(EngineOpen::new(path, v13::SCHEMA_VERSION))
        .expect("open v13 archive fixture");
    let records = database
        .register_table(layout.records_descriptor())
        .expect("register archive records");
    database
        .assert(Assertion::new(
            records,
            legacy_record(
                "formerly-private-17",
                legacy_entry(
                    "archived retained substance",
                    v13::Magnitude::Medium,
                    v13::Magnitude::High,
                    &["retired-archive-topic"],
                    v13::Magnitude::VeryHigh,
                ),
            ),
        ))
        .expect("assert archived v13 record");
}

fn digest(path: &Path) -> blake3::Hash {
    blake3::hash(&fs::read(path).expect("read fixture bytes"))
}

#[test]
fn v13_projection_discards_retired_data_and_preserves_live_archive_and_rollback() {
    let fixture = TempDir::new().expect("tempdir");
    let live = fixture.path().join("spirit.sema");
    let archive = fixture.path().join("spirit.archive.sema");
    let journal_v6 = fixture.path().join("spirit.guardian.v6.sema");
    seed_v13_live(&live);
    seed_v13_archive(&archive);
    fs::write(&journal_v6, b"exact-v6-guardian-rollback-bytes").expect("seed v6 guardian bytes");
    let live_v13_digest = digest(&live);
    let archive_v13_digest = digest(&archive);
    let journal_v6_digest = digest(&journal_v6);

    let output = StoreMigration::new(StoreMigrationRequest::new(live.display().to_string()))
        .run()
        .expect("migrate v13 fixture");
    let StoreMigrationOutput::Migrated(completed) = output else {
        panic!("expected migration")
    };
    assert_eq!(completed.record_count(), 2);

    let store = Store::open(&live).expect("open migrated v14 store");
    assert_eq!(store.store_schema_version(), 14);
    let zero = store
        .entry_by_identifier("zero-certainty")
        .expect("query zero-certainty")
        .expect("zero-certainty row survives");
    assert_eq!(
        zero.description.payload(),
        "zero certainty survives as an ordinary record"
    );
    assert_eq!(zero.importance.payload(), &Magnitude::Low);
    let formerly_private = store
        .entry_by_identifier("formerly-private")
        .expect("query formerly private")
        .expect("formerly private row survives");
    assert_eq!(
        formerly_private.description.payload(),
        "formerly private survives as an ordinary record"
    );
    assert_eq!(formerly_private.importance.payload(), &Magnitude::High);

    let migrations = store.migrations().expect("read v14 migration receipt");
    assert_eq!(migrations.len(), 1);
    assert_eq!(*migrations[0].source_schema_version.payload(), 13);
    assert_eq!(*migrations[0].migrated_record_count.payload(), 2);
    let registrations = store.engine_handle().list_tables();
    let family_names = registrations
        .iter()
        .map(|registration| registration.identity().family().as_str())
        .collect::<Vec<_>>();
    assert_eq!(family_names.len(), 2);
    assert!(family_names.contains(&"RecordsFamily"));
    assert!(family_names.contains(&"MigrationsFamily"));
    let log = store.versioned_log().expect("read fresh v14 log");
    assert_eq!(log.len(), 3, "two projected assertions plus one receipt");
    assert!(
        log.iter()
            .flat_map(|entry| entry.operations())
            .all(|operation| operation.operation() == signal_sema::SemaOperation::Assert)
    );

    let mut archive_database =
        SemaDatabase::open(EngineOpen::new(&archive, SchemaVersion::new(14)))
            .expect("open projected archive");
    let archived_records = archive_database
        .register_table(RecordFamily::records_family())
        .expect("register projected archive records");
    let archived = archive_database
        .match_records(QueryPlan::all(archived_records))
        .expect("enumerate projected archive");
    assert_eq!(archived.records().len(), 1);
    assert_eq!(
        archived.records()[0].record_identifier.payload(),
        "formerly-private-17"
    );
    assert_eq!(
        archived.records()[0].entry.description.payload(),
        "archived retained substance"
    );
    assert_eq!(
        archived.records()[0].entry.importance.payload(),
        &Magnitude::VeryHigh
    );

    let rollback = fixture.path().join("spirit.schema-13-rollback");
    assert_eq!(
        fs::metadata(&rollback)
            .expect("rollback metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(digest(&rollback.join("live.v13.sema")), live_v13_digest);
    assert_eq!(
        digest(&rollback.join("archive.v13.sema")),
        archive_v13_digest
    );
    assert_eq!(
        digest(&rollback.join("guardian.v6.sema")),
        journal_v6_digest
    );
    v13::LiveReader::open(rollback.join("live.v13.sema")).expect("rollback live reopens as v13");
    v13::ArchiveReader::open(rollback.join("archive.v13.sema"))
        .expect("rollback archive reopens as v13");

    let current_bytes = fs::read(&live).expect("read v14 live bytes");
    assert!(
        !current_bytes
            .windows(b"unique-retired-alias".len())
            .any(|window| window == b"unique-retired-alias")
    );
    assert_eq!(
        store
            .guardian_decision_count()
            .expect("open fresh v7 journal"),
        0
    );
    assert!(fixture.path().join("spirit.guardian.v7.sema").exists());
    assert_eq!(
        digest(&journal_v6),
        journal_v6_digest,
        "v6 journal remains untouched"
    );

    drop(archived);
    drop(archive_database);
    drop(store);
    let second = StoreMigration::new(StoreMigrationRequest::new(live.display().to_string()))
        .run()
        .expect("second migration is current no-op");
    let StoreMigrationOutput::Current(completed) = second else {
        panic!("expected current no-op")
    };
    assert_eq!(completed.record_count(), 2);
}

#[test]
fn corrupt_or_wrong_generation_source_fails_without_rewriting_it() {
    let fixture = TempDir::new().expect("tempdir");
    let live = fixture.path().join("spirit.sema");
    fs::write(&live, b"not-a-spirit-store").expect("seed corrupt source");
    let before = digest(&live);
    assert!(
        StoreMigration::new(StoreMigrationRequest::new(live.display().to_string()))
            .run()
            .is_err()
    );
    assert_eq!(digest(&live), before);
    assert!(!fixture.path().join("spirit.schema-13-rollback").exists());
}
