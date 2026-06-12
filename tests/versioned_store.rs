//! Data-loss-protection witnesses for the versioned store pilot.
//!
//! The versioned commit log is the authoritative history of the intent
//! corpus; the table store is its fold. These witnesses prove the arc's
//! point at the daemon storage surface: a checkpoint plus the log suffix
//! restores a fresh store whose query surface — records, identifiers,
//! markers — is identical to the original, and every durable write is
//! covered by a decodable versioned log operation.

use std::path::PathBuf;

use spirit::{
    Store,
    schema::signal::{
        Certainty, Description, Domain, Domains, Entry, Importance, Kind, Magnitude, Privacy,
        Referent, Referents,
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

fn justification(reasoning: &str) -> spirit::schema::signal::Justification {
    spirit::schema::signal::Justification {
        testimony: spirit::schema::signal::Testimony::new(vec![
            spirit::schema::signal::VerbatimQuote {
                quote_text: spirit::schema::signal::QuoteText::new(reasoning),
                antecedent: None,
            },
        ]),
        reasoning: spirit::schema::signal::Reasoning::new(reasoning),
    }
}

fn entry(description: &str, referents: Vec<Referent>) -> Entry {
    Entry {
        domains: Domains::new(vec![Domain::Information(
            spirit::schema::signal::Information::Documentation,
        )]),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: Certainty::new(Magnitude::High),
        importance: Importance::new(Magnitude::Medium),
        privacy: Privacy::new(Magnitude::Zero),
        referents: Referents::new(referents),
    }
}

/// Checkpoint + suffix into a fresh store restores the identical query
/// surface: same identifiers, same entries, same record count, same
/// database marker (commit sequence AND content digest).
#[test]
fn checkpoint_and_suffix_restore_an_identical_store() {
    let fixture = Fixture::new();
    let source = Store::open(fixture.database_path("source")).expect("open source store");

    source
        .register_referent(spirit::schema::signal::ReferentRegistration {
            referent: Referent::new("sema-engine"),
            aliases: Referents::new(vec![Referent::new("sema engine")]),
            justification: justification("witness referent registration"),
        })
        .expect("register referent");
    let first = source
        .record_entry(entry(
            "the log is authoritative",
            vec![Referent::new("sema-engine")],
        ))
        .expect("record first entry");
    source
        .import_record(
            String::from("hj63"),
            entry("imported identifier survives", Vec::new()),
        )
        .expect("import keyed record");

    source.checkpoint().expect("write checkpoint");

    // Post-checkpoint suffix: one more record so the import has both a
    // checkpoint body and a live suffix to carry.
    let second = source
        .record_entry(entry("suffix entry rides the log", Vec::new()))
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
        .expect("import into fresh store");

    // The daemon-level query surface is identical.
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
            "restored entry differs for identifier {identifier}",
        );
    }
    assert_eq!(restored.database_marker(), source.database_marker());
}

/// Every durable write is covered by the versioned log: the log decodes
/// through the generated closed family sum back to exactly the rows the
/// query surface serves.
#[test]
fn versioned_log_covers_every_durable_write() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.database_path("covered")).expect("open store");

    let receipt = store
        .record_entry(entry("covered by the log", Vec::new()))
        .expect("record entry");
    store
        .register_referent(spirit::schema::signal::ReferentRegistration {
            referent: Referent::new("spirit"),
            aliases: Referents::new(Vec::new()),
            justification: justification("witness referent registration"),
        })
        .expect("register referent");

    let log = store.versioned_log().expect("read versioned log");
    let operations: Vec<_> = log
        .iter()
        .flat_map(|log_entry| log_entry.operations().iter())
        .collect();
    assert_eq!(operations.len(), 2);
    for operation in operations {
        let payload = operation.payload().bytes().expect("record payload bytes");
        match spirit::schema::sema::RecordFamily::decode(operation.family(), payload)
            .expect("decode logged payload")
        {
            spirit::schema::sema::RecordFamily::RecordsFamily(record) => {
                assert_eq!(&record.record_identifier, &receipt.record_identifier);
                let stored = store
                    .entry_by_identifier(record.record_identifier.payload())
                    .expect("query logged record")
                    .expect("logged record exists");
                assert_eq!(stored, record.entry);
            }
            spirit::schema::sema::RecordFamily::ReferentsFamily(referent) => {
                assert_eq!(referent.referent, Referent::new("spirit"));
            }
            spirit::schema::sema::RecordFamily::MigrationsFamily(migration) => {
                panic!("no migration was recorded, got {migration:?}")
            }
        }
    }
}
