//! Data-loss-protection witnesses for the versioned store pilot.
//!
//! The versioned commit log is the authoritative history of the intent
//! corpus; the table store is its fold. These witnesses prove the arc's
//! point at the daemon storage surface: a checkpoint plus the log suffix
//! restores a fresh store whose query surface — records, identifiers,
//! markers — is identical to the original, and every durable write is
//! covered by a decodable versioned log operation.

use std::path::PathBuf;

use signal_sema::SemaOperation;
use spirit::{
    Store,
    schema::{
        sema::{
            self, RecordFamily, SemaEngine, WriteInput as SemaWriteInput,
            WriteOutput as SemaWriteOutput,
        },
        signal::{
            Certainty, CertaintyChange, Description, Domain, Domains, Entry, Importance, Kind,
            Magnitude, Privacy, Referent, ReferentRegistration, Referents, Removal,
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

fn sema_write(input: SemaWriteInput, offset: u64) -> sema::Sema<sema::WriteInput> {
    input.with_origin_route(sema::OriginRoute::new(9_000_000 + offset))
}

/// Mutations land in the versioned log with the shape replay depends on:
/// a keyed operation labeled `Mutate` whose payload decodes through the
/// generated closed family sum to the post-mutation row — for both a
/// record certainty change and a referent alias merge, driven through the
/// generated SEMA write surface the daemon itself uses. A log-shape
/// regression on the mutation path fails here, not in a later replay.
#[test]
fn versioned_log_witnesses_mutation_payloads() {
    let fixture = Fixture::new();
    let mut store = Store::open(fixture.database_path("mutated")).expect("open store");

    store
        .register_referent(ReferentRegistration {
            referent: Referent::new("sema-engine"),
            aliases: Referents::new(vec![Referent::new("sema engine")]),
            justification: justification("witness referent registration"),
        })
        .expect("register referent");
    let receipt = store
        .record_entry(entry("mutation target", Vec::new()))
        .expect("record entry");

    let changed = SemaEngine::apply(
        &mut store,
        sema_write(
            SemaWriteInput::change_certainty(CertaintyChange {
                record_identifier: receipt.record_identifier.clone(),
                certainty: Certainty::new(Magnitude::Zero),
            }),
            1,
        ),
    );
    match changed.root() {
        SemaWriteOutput::CertaintyChanged(_) => {}
        other => panic!("expected CertaintyChanged receipt, got {other:?}"),
    }
    let merged = SemaEngine::apply(
        &mut store,
        sema_write(
            SemaWriteInput::register_referent(ReferentRegistration {
                referent: Referent::new("sema-engine"),
                aliases: Referents::new(vec![Referent::new("semantic engine")]),
                justification: justification("witness referent alias merge"),
            }),
            2,
        ),
    );
    match merged.root() {
        SemaWriteOutput::ReferentRegistered(_) => {}
        other => panic!("expected ReferentRegistered receipt, got {other:?}"),
    }

    let log = store.versioned_log().expect("read versioned log");
    let mutations: Vec<_> = log
        .iter()
        .flat_map(|log_entry| log_entry.operations().iter())
        .filter(|operation| operation.operation() == SemaOperation::Mutate)
        .collect();
    assert_eq!(
        mutations.len(),
        2,
        "expected exactly the certainty-change and alias-merge mutations",
    );
    let mut record_mutations = 0;
    let mut referent_mutations = 0;
    for mutation in mutations {
        let payload = mutation
            .payload()
            .bytes()
            .expect("mutation payload carries record bytes");
        match RecordFamily::decode(mutation.family(), payload).expect("decode mutated payload") {
            RecordFamily::RecordsFamily(record) => {
                record_mutations += 1;
                assert_eq!(&record.record_identifier, &receipt.record_identifier);
                assert_eq!(record.entry.certainty, Certainty::new(Magnitude::Zero));
                assert_eq!(
                    mutation.key().map(|key| key.to_owned_string()),
                    Some(receipt.record_identifier.payload().clone()),
                    "mutation is keyed to the mutated record",
                );
            }
            RecordFamily::ReferentsFamily(referent) => {
                referent_mutations += 1;
                assert_eq!(referent.referent, Referent::new("sema-engine"));
                for alias in ["sema engine", "semantic engine"] {
                    assert!(
                        referent.aliases.payload().contains(&Referent::new(alias)),
                        "merged referent row carries alias {alias}",
                    );
                }
            }
            RecordFamily::MigrationsFamily(migration) => {
                panic!("no migration was recorded, got {migration:?}")
            }
        }
    }
    assert_eq!(record_mutations, 1, "one certainty-change mutation");
    assert_eq!(referent_mutations, 1, "one referent alias-merge mutation");
}

/// Retractions land in the versioned log as the engine's keyed tombstone:
/// the operation is labeled `Retract`, addressed to the removed record's
/// key in the records family, and carries no record bytes. A log-shape
/// regression on the removal path fails here, not in a later replay.
#[test]
fn versioned_log_witnesses_retraction_tombstones() {
    let fixture = Fixture::new();
    let mut store = Store::open(fixture.database_path("retracted")).expect("open store");

    let kept = store
        .record_entry(entry("kept neighbor", Vec::new()))
        .expect("record kept entry");
    let removed = store
        .record_entry(entry("removal target", Vec::new()))
        .expect("record removal target");

    let removal = SemaEngine::apply(
        &mut store,
        sema_write(
            SemaWriteInput::remove(Removal {
                record_identifier: removed.record_identifier.clone(),
                justification: justification("witness removal"),
            }),
            1,
        ),
    );
    match removal.root() {
        SemaWriteOutput::Removed(receipt) => {
            assert_eq!(receipt.payload().payload(), &removed.record_identifier);
        }
        other => panic!("expected Removed receipt, got {other:?}"),
    }

    let log = store.versioned_log().expect("read versioned log");
    let retractions: Vec<_> = log
        .iter()
        .flat_map(|log_entry| log_entry.operations().iter())
        .filter(|operation| operation.operation() == SemaOperation::Retract)
        .collect();
    assert_eq!(retractions.len(), 1, "expected exactly one retraction");
    let retraction = retractions[0];
    assert!(
        retraction.payload().is_tombstone(),
        "retraction carries the engine tombstone, not record bytes",
    );
    assert_eq!(retraction.payload().bytes(), None);
    assert_eq!(
        retraction.key().map(|key| key.to_owned_string()),
        Some(removed.record_identifier.payload().clone()),
        "tombstone is keyed to the removed record",
    );
    assert_eq!(
        retraction.family().schema_hash(),
        RecordFamily::records_family().schema_hash(),
        "tombstone lands in the records family",
    );

    // The fold of that log matches: the removed record is gone from the
    // query surface while its neighbor survives.
    assert_eq!(
        store
            .entry_by_identifier(removed.record_identifier.payload())
            .expect("query removed identifier"),
        None,
    );
    assert!(
        store
            .entry_by_identifier(kept.record_identifier.payload())
            .expect("query kept identifier")
            .is_some(),
    );
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
