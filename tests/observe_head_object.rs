//! `ObserveHeadObject` returns the REAL versioned-log head BODY of a seeded
//! record, and re-deriving the digest from that body reproduces the head
//! `ObserveHead` returns.
//!
//! The two-VM criome-auth witness must forward the EXACT content-addressed
//! record body that criome authenticates and the mirror durably lands — the
//! `rkyv`-serialized `VersionedCommitLogEntry` the production
//! `ComponentShipper::envelope_for_entry` ships — not a placeholder. The sibling
//! `ObserveHead` op surfaces the head DIGEST; this op surfaces the head BODY, so
//! the witness sources the real octets from Spirit, decodes the hex to a file,
//! and feeds it to `router-forward-witness` as the forwarded `Append` payload.
//!
//! These witnesses prove the owner-only meta `ObserveHeadObject` op:
//!
//!  1. an empty store honestly reports NO head object;
//!  2. the surfaced hex decodes to byte-for-byte the store's own serialized head
//!     entry (`versioned_log_head_object`) — the same octets the shipper ships,
//!     never an invented format;
//!  3. decoding the hex and reconstructing through the PUBLIC
//!     `VersionedCommitLogEntry::new` (which recomputes the digest from the
//!     decoded fields, never trusting a stored digest) reproduces the store's
//!     own `versioned_log_head` — the SAME value `ObserveHead` returns. So head
//!     and body are consistent by construction: re-hashing the body the witness
//!     lands reproduces the head the witness forwards.

#![cfg(feature = "nota-text")]

use sema_engine::VersionedCommitLogEntry;
use spirit::schema::meta_signal::{Input as MetaInput, Output as MetaOutput};
use spirit::{Engine, Store};
use tempfile::TempDir;

/// The EXACT owner-only meta `Import` the criome-auth witness sends to seed
/// node-a's spirit daemon (mkCriomeAuthWitnessTest `importNota`).
const WITNESS_IMPORT_NOTA: &str = "(Import [(witness-record-1 ([(Technology (Software (Programming CodeGeneration)))] Decision [criome auth witness record] Low Zero))])";

fn open_engine() -> (TempDir, Engine) {
    let directory = tempfile::tempdir().expect("create sandbox");
    let store = Store::open(directory.path().join("spirit.sema")).expect("open store");
    (directory, Engine::new(store))
}

/// Seed the witness's exact record through the meta `Import` path.
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

/// The lowercase-hex head body carried by an `ObserveHeadObject` reply, or
/// `None` when the store has no versioned-log head yet.
fn observed_head_object_hex(engine: &Engine) -> Option<String> {
    let MetaOutput::HeadObjectObserved(observed) = engine.observe_head_object() else {
        panic!("ObserveHeadObject must reply HeadObjectObserved");
    };
    observed
        .payload()
        .selected_head_object
        .payload()
        .as_ref()
        .map(|object| object.payload().clone())
}

/// The lowercase-hex head DIGEST carried by an `ObserveHead` reply.
fn observed_head_digest_hex(engine: &Engine) -> Option<String> {
    let MetaOutput::HeadObserved(observed) = engine.observe_head() else {
        panic!("ObserveHead must reply HeadObserved");
    };
    observed
        .payload()
        .selected_head_digest
        .payload()
        .as_ref()
        .map(|digest| digest.payload().clone())
}

/// Decode an even-length lowercase-hex string into its octets — the exact
/// decode the witness testScript performs (`xxd -r -p`) before writing the
/// `ENTRY_BODY_PATH` file.
fn decode_hex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0, "hex body has even length: {}", hex.len());
    (0..hex.len() / 2)
        .map(|index| {
            u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("valid hex byte")
        })
        .collect()
}

#[test]
fn empty_store_reports_no_head_object() {
    let (_directory, engine) = open_engine();
    assert_eq!(
        observed_head_object_hex(&engine),
        None,
        "an empty versioned log has no head body to forward or land"
    );
}

#[test]
fn observe_head_object_returns_the_real_body_that_rehashes_to_the_head() {
    let (_directory, mut engine) = open_engine();
    seed_witness_record(&mut engine);

    let body_hex = observed_head_object_hex(&engine).expect("a seeded store reports its head body");

    // Wire form: lowercase hex, decodable by the witness's `xxd -r -p`.
    assert!(
        body_hex
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character)),
        "head body is lowercase hex"
    );
    let body = decode_hex(&body_hex);

    // (2) The hex decodes to byte-for-byte the store's own serialized head
    // entry — the same octets `ComponentShipper::envelope_for_entry` ships.
    assert_eq!(
        Some(body.clone()),
        engine
            .store()
            .versioned_log_head_object()
            .expect("read the serialized head entry"),
        "the surfaced hex decodes to exactly the store's serialized head entry"
    );

    // (3) Decode + reconstruct through sema-engine's own content-addressing:
    // `new` recomputes the digest from the decoded fields, never trusting a
    // stored digest.
    let decoded = rkyv::from_bytes::<VersionedCommitLogEntry, rkyv::rancor::Error>(&body)
        .expect("the head body is a genuine rkyv VersionedCommitLogEntry");
    let rederived = VersionedCommitLogEntry::new(
        decoded.store_name().clone(),
        decoded.schema_hash(),
        decoded.commit_sequence(),
        decoded.snapshot(),
        decoded.previous_entry_digest(),
        decoded.operations().clone(),
    );

    let head = engine
        .store()
        .versioned_log_head()
        .expect("read versioned-log head")
        .expect("a seeded store has a head");
    assert_eq!(
        rederived.entry_digest(),
        head,
        "re-deriving the digest from the ObserveHeadObject body reproduces the store's head"
    );

    // The re-derived head equals the SAME hex `ObserveHead` reports: the witness
    // forwards `ObserveHead`'s digest as HEAD_DIGEST_HEX and lands this body, and
    // the two agree because both come from this one seeded entry.
    let head_hex = observed_head_digest_hex(&engine).expect("ObserveHead reports the head");
    assert_eq!(
        rederived.entry_digest().to_string(),
        head_hex,
        "the body re-hashes to the exact digest ObserveHead returns"
    );

    // Evidence trail: the head digest the seeded witness daemon forwards, and
    // the length of the body the witness lands.
    eprintln!("OBSERVE_HEAD_OBJECT head digest = {head_hex}");
    eprintln!("OBSERVE_HEAD_OBJECT body octets = {}", body.len());
}
