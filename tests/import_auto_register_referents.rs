//! The owner import bypass auto-registers a record's referents (kebab-only).
//!
//! `import_record` is the guardian-bypassing owner path. These witnesses prove
//! it (1) registers a brand-new referent the imported entry carries, so a
//! corpus import is self-contained rather than failing on UnregisteredReferent,
//! and (2) refuses a non-kebab-case referent name, the format the store accepts
//! for every new referent on every path.
//!
//! That `import_record` returns Ok is itself the proof of registration: it
//! canonicalizes the entry after the auto-register loop, which errors
//! `UnregisteredReferent` for any referent not in the store.

use spirit::{
    Store, StoreError,
    schema::signal::{
        Certainty, Description, Domain, Domains, Entry, Importance, Information, Kind, Magnitude,
        Privacy, Referent, Referents,
    },
};
use tempfile::TempDir;

fn entry(description: &str, referents: Vec<Referent>) -> Entry {
    Entry {
        domains: Domains::new(vec![Domain::Information(Information::Documentation)]),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: Certainty::new(Magnitude::High),
        importance: Importance::new(Magnitude::Minimum),
        privacy: Privacy::new(Magnitude::Zero),
        referents: Referents::new(referents),
    }
}

fn open_store() -> (TempDir, Store) {
    let directory = tempfile::tempdir().expect("create sandbox");
    let store = Store::open(directory.path().join("spirit.sema")).expect("open store");
    (directory, store)
}

#[test]
fn import_auto_registers_a_new_kebab_referent() {
    let (_directory, store) = open_store();

    // Without auto-registration this would fail UnregisteredReferent; Ok proves
    // the referent was registered before the entry was canonicalized and written.
    store
        .import_record(
            String::from("imp1"),
            entry(
                "owner import carries its referent",
                vec![Referent::new("sema-engine")],
            ),
        )
        .expect("import auto-registers the kebab referent and writes the record");

    // A second record reusing the now-registered referent also succeeds.
    store
        .import_record(
            String::from("imp2"),
            entry(
                "a second record reuses the now-registered referent",
                vec![Referent::new("sema-engine")],
            ),
        )
        .expect("reuse of the auto-registered referent succeeds");
}

#[test]
fn import_upserts_an_existing_record() {
    let (_directory, store) = open_store();

    store
        .import_record(
            String::from("dup1"),
            entry("original text", vec![Referent::new("spirit")]),
        )
        .expect("first import inserts");
    // Re-importing the same id overwrites in place (owner curation), rather than
    // failing on the insert-only assert.
    store
        .import_record(
            String::from("dup1"),
            entry(
                "curated replacement text",
                vec![Referent::new("spirit"), Referent::new("sema-engine")],
            ),
        )
        .expect("second import of the same id upserts in place");
}

#[test]
fn import_refuses_a_non_kebab_referent() {
    let (_directory, store) = open_store();

    let result = store.import_record(
        String::from("imp3"),
        entry(
            "capitalized referent is refused",
            vec![Referent::new("SemaEngine")],
        ),
    );
    assert!(
        matches!(&result, Err(StoreError::NonKebabReferent(name)) if name == "SemaEngine"),
        "a non-kebab referent name is refused on the import path, got {result:?}",
    );
}
