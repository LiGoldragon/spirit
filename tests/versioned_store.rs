//! Durable v14 log, checkpoint, and lifecycle witnesses.

use std::path::PathBuf;

use signal_sema::SemaOperation;
use spirit::{
    SPIRIT_STORE_NAME, Store,
    schema::{
        sema::RecordFamily,
        signal::{
            Description, Domain, Domains, Entry, Importance, Kind, Magnitude, QuoteText, Reasoning,
            Retirement, Testimony, VerbatimQuote,
        },
    },
};
use tempfile::TempDir;

struct Fixture {
    directory: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().expect("create versioned store sandbox"),
        }
    }

    fn database_path(&self, name: &str) -> PathBuf {
        self.directory.path().join(format!("{name}.sema"))
    }
}

fn entry(description: &str) -> Entry {
    Entry {
        domains: Domains::new(vec![Domain::Information(
            spirit::schema::signal::Information::Documentation,
        )]),
        kind: Kind::Decision,
        description: Description::new(description),
        importance: Importance::new(Magnitude::Medium),
    }
}

fn justification(reasoning: &str) -> spirit::schema::signal::Justification {
    spirit::schema::signal::Justification {
        testimony: Testimony::new(vec![VerbatimQuote::new(QuoteText::new(reasoning), None)]),
        reasoning: Reasoning::new(reasoning),
    }
}

#[test]
fn checkpoint_and_suffix_restore_the_identical_v14_store() {
    assert_eq!(SPIRIT_STORE_NAME, "spirit:sema:v14");
    let fixture = Fixture::new();
    let source = Store::open(fixture.database_path("source")).expect("open source store");

    let first = source
        .record_entry(entry("the v14 log is authoritative"))
        .expect("record first entry");
    source
        .import_record(String::from("hj63"), entry("stable imported identifier"))
        .expect("import keyed record");
    source.checkpoint().expect("write checkpoint");
    let second = source
        .record_entry(entry("suffix entry rides the v14 log"))
        .expect("record suffix entry");

    let checkpoint = source
        .latest_checkpoint()
        .expect("load checkpoint")
        .expect("checkpoint exists");
    let suffix = source
        .versioned_log_from(checkpoint.metadata().covered().last().next())
        .expect("read log suffix");
    assert_eq!(suffix.len(), 1);

    let restored = Store::import(fixture.database_path("restored"), checkpoint, suffix)
        .expect("import into fresh v14 store");
    assert_eq!(restored.len(), source.len());
    for identifier in [
        first.record_identifier.payload().as_str(),
        second.record_identifier.payload().as_str(),
        "hj63",
    ] {
        assert_eq!(
            restored
                .entry_by_identifier(identifier)
                .expect("query restored entry"),
            source
                .entry_by_identifier(identifier)
                .expect("query source entry"),
        );
    }
    assert_eq!(restored.database_marker(), source.database_marker());
}

#[test]
fn versioned_log_contains_only_current_record_family_payloads() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.database_path("covered")).expect("open store");
    let receipt = store
        .record_entry(entry("covered by the v14 log"))
        .expect("record entry");

    let log = store.versioned_log().expect("read versioned log");
    let operations: Vec<_> = log
        .iter()
        .flat_map(|log_entry| log_entry.operations().iter())
        .collect();
    assert_eq!(operations.len(), 1);
    let operation = operations[0];
    assert_eq!(operation.operation(), SemaOperation::Assert);
    let payload = operation.payload().bytes().expect("record payload bytes");
    match RecordFamily::decode(operation.family(), payload).expect("decode logged payload") {
        RecordFamily::RecordsFamily(record) => {
            assert_eq!(&record.record_identifier, &receipt.record_identifier);
            assert_eq!(record.entry, entry("covered by the v14 log"));
        }
        RecordFamily::MigrationsFamily(migration) => {
            panic!("ordinary record unexpectedly decoded as migration {migration:?}")
        }
    }
}

#[test]
fn retire_archives_before_emitting_one_retraction_tombstone() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.database_path("retracted")).expect("open store");
    let kept = store
        .record_entry(entry("kept neighbor"))
        .expect("record kept entry");
    let removed = store
        .record_entry(entry("retirement target"))
        .expect("record retirement target");

    store
        .retire(Retirement {
            record_identifier: removed.record_identifier.clone(),
            justification: justification("explicit retirement"),
        })
        .expect("retire target")
        .expect("target exists");

    let log = store.versioned_log().expect("read versioned log");
    let retractions: Vec<_> = log
        .iter()
        .flat_map(|log_entry| log_entry.operations().iter())
        .filter(|operation| operation.operation() == SemaOperation::Retract)
        .collect();
    assert_eq!(retractions.len(), 1);
    assert!(retractions[0].payload().is_tombstone());
    assert_eq!(
        retractions[0].key().map(|key| key.to_owned_string()),
        Some(removed.record_identifier.payload().clone())
    );
    assert!(fixture.database_path("retracted.archive").exists());
    assert!(
        store
            .entry_by_identifier(removed.record_identifier.payload())
            .expect("query retired id")
            .is_none()
    );
    assert!(
        store
            .entry_by_identifier(kept.record_identifier.payload())
            .expect("query kept id")
            .is_some()
    );
}
