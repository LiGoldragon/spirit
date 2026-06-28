//! `ObserveHead` returns the REAL versioned-log content head of a seeded record.
//!
//! The criome-auth witness must forward the content-addressed head that criome
//! authenticates and the mirror durably lands — NOT a synthetic stand-in. The
//! prior witness forwarded `sha256("witness-record-1:criome auth witness
//! record")`, a hash of two Nix string literals that was never read from the
//! daemon and never tied to the stored record.
//!
//! These witnesses prove the owner-only meta `ObserveHead` op returns exactly
//! the store's versioned-log head `EntryDigest`:
//!
//!  1. an empty store honestly reports NO head;
//!  2. after seeding the witness's EXACT record (parsed from the same `Import`
//!     NOTA the witness sends over the meta socket), `ObserveHead` reports a
//!     head that is byte-for-byte the engine's own `versioned_log_head()` — so
//!     it is genuinely tied to the stored content, not recomputed;
//!  3. the head is 64 lowercase hex characters — the exact `HEAD_DIGEST_HEX`
//!     form `router-forward-witness` ingests and `criome`'s `ObjectDigest`
//!     carries — and is NOT the old synthetic sha256 stand-in;
//!  4. the head is content-deterministic: a fresh store seeded with the same
//!     record yields the same head, so the witness daemon (same spirit build,
//!     same record) forwards this same value.

#![cfg(feature = "nota-text")]

use spirit::schema::meta_signal::{Input as MetaInput, Output as MetaOutput};
use spirit::{Engine, Store};
use tempfile::TempDir;

/// The EXACT owner-only meta `Import` the criome-auth witness sends to seed
/// node-a's spirit daemon (mkCriomeAuthWitnessTest `importNota`).
const WITNESS_IMPORT_NOTA: &str = "(Import [(witness-record-1 ([(Technology (Software (Programming CodeGeneration)))] Decision [criome auth witness record] High Low Zero []))])";

/// The synthetic stand-in the prior witness forwarded:
/// `printf '%s' 'witness-record-1:criome auth witness record' | sha256sum`.
/// The real content head must NOT equal this.
const SYNTHETIC_STAND_IN: &str = "cdc1c22fea273efbade8385bfa0e5c73899bb66632b96949e0952fc77891b718";

fn open_engine() -> (TempDir, Engine) {
    let directory = tempfile::tempdir().expect("create sandbox");
    let store = Store::open(directory.path().join("spirit.sema")).expect("open store");
    (directory, Engine::new(store))
}

/// Seed the witness's exact record through the meta `Import` path and return the
/// engine that now owns the stored record.
fn seed_witness_record(engine: &mut Engine) {
    let MetaInput::Import(import) = WITNESS_IMPORT_NOTA
        .parse::<MetaInput>()
        .expect("parse witness import NOTA")
    else {
        panic!("witness NOTA must be an Import");
    };
    let receipt = engine.import(import.into_payload());
    assert!(
        matches!(receipt, MetaOutput::Imported(_)),
        "meta Import must seed the record, got {receipt:?}"
    );
}

/// The head hex carried by an `ObserveHead` reply, or `None` when the store has
/// no versioned-log head yet.
fn observed_head_hex(engine: &Engine) -> Option<String> {
    let MetaOutput::HeadObserved(observed) = engine.observe_head() else {
        panic!("ObserveHead must reply HeadObserved");
    };
    observed
        .payload()
        .selected_head_digest
        .payload()
        .as_ref()
        .map(|head| head.payload().clone())
}

#[test]
fn empty_store_reports_no_head() {
    let (_directory, engine) = open_engine();
    assert_eq!(
        observed_head_hex(&engine),
        None,
        "an empty versioned log has no head to authorize or land"
    );
}

#[test]
fn observe_head_returns_the_real_stored_content_head() {
    let (_directory, mut engine) = open_engine();
    seed_witness_record(&mut engine);

    let observed = observed_head_hex(&engine).expect("a seeded store reports its head");

    // (2) The op's head IS the engine's own versioned-log head — read from the
    // same durable log, not recomputed. This is the load-bearing tie to the
    // stored record.
    let stored_head = engine
        .store()
        .versioned_log_head()
        .expect("read versioned-log head")
        .expect("seeded store has a head");
    assert_eq!(
        observed,
        stored_head.to_string(),
        "ObserveHead must report the store's actual versioned-log EntryDigest"
    );

    // (3) Wire form: 64 lowercase hex chars (HEAD_DIGEST_HEX / criome ObjectDigest).
    assert_eq!(
        observed.len(),
        64,
        "head digest is 64 hex chars: {observed}"
    );
    assert!(
        observed
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character)),
        "head digest is lowercase hex: {observed}"
    );

    // ... and it is NOT the prior synthetic sha256 stand-in.
    assert_ne!(
        observed, SYNTHETIC_STAND_IN,
        "the real content head must not be the synthetic stand-in"
    );
}

#[test]
fn the_head_is_content_deterministic() {
    let (_first_directory, mut first) = open_engine();
    seed_witness_record(&mut first);
    let first_head = observed_head_hex(&first).expect("first store reports its head");

    let (_second_directory, mut second) = open_engine();
    seed_witness_record(&mut second);
    let second_head = observed_head_hex(&second).expect("second store reports its head");

    // Same spirit build, same record, same content-addressing: the witness
    // daemon forwards exactly this head for its seeded record.
    assert_eq!(
        first_head, second_head,
        "the content head is deterministic across fresh stores"
    );
}
