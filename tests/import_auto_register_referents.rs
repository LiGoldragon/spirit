//! Owner import writes and upserts current-shape records.
//!
//! Entries no longer carry embedded referents, so import no longer derives
//! referent registrations from record payloads. The owner import path still
//! needs a focused witness for keyed corpus import and curated replacement.

use spirit::{
    Store,
    schema::signal::{
        Description, Domain, Domains, Entry, Importance, Information, Kind, Magnitude, Privacy,
    },
};
use tempfile::TempDir;

fn entry(description: &str) -> Entry {
    Entry {
        domains: Domains::new(vec![Domain::Information(Information::Documentation)]),
        kind: Kind::Decision,
        description: Description::new(description),
        importance: Importance::new(Magnitude::Minimum),
        privacy: Privacy::new(Magnitude::Zero),
    }
}

fn open_store() -> (TempDir, Store) {
    let directory = tempfile::tempdir().expect("create sandbox");
    let store = Store::open(directory.path().join("spirit.sema")).expect("open store");
    (directory, store)
}

#[test]
fn import_writes_a_keyed_record() {
    let (_directory, store) = open_store();

    store
        .import_record(
            String::from("imp1"),
            entry("owner import writes current entry"),
        )
        .expect("import writes the record");

    let imported = store
        .entry_by_identifier("imp1")
        .expect("query imported record")
        .expect("record exists");
    assert_eq!(
        imported.description.payload(),
        "owner import writes current entry"
    );
}

#[test]
fn import_upserts_an_existing_record() {
    let (_directory, store) = open_store();

    store
        .import_record(String::from("dup1"), entry("original text"))
        .expect("first import inserts");
    store
        .import_record(String::from("dup1"), entry("curated replacement text"))
        .expect("second import of the same id upserts in place");

    let imported = store
        .entry_by_identifier("dup1")
        .expect("query imported record")
        .expect("record exists");
    assert_eq!(imported.description.payload(), "curated replacement text");
}
