//! The quorum-gated authorized-apply ingress witness (Spirit `xhwa`, piece 4).
//!
//! An arriving record — a real content-addressed `VersionedCommitLogEntry`
//! produced by a SOURCE store, paired with an Evidence bound to its digest —
//! lands LIVE into a fresh TARGET store on an `Authorized` verdict and is
//! immediately observable; a non-`Authorized` verdict or a re-hash-mismatched
//! body is refused fail-closed. The criome socket round-trip is exercised by the
//! integration slice (nbmq.3) against a real local criome; here the decision is
//! supplied directly so the fail-closed gate is proven against a
//! locally-assembled Evidence without a live daemon.

use sema_engine::VersionedCommitLogEntry;
use signal_criome::{
    AttestedMoment, AttestedMomentProposition, ComponentKind, EvaluationDecision, Evidence,
    OperationDigest, RequiredSignatureThreshold, TimeWindow, TimestampNanos,
};
use spirit::Store;
use spirit::apply_ingress::{AuthorizedApplyOutcome, PreparedAuthorizedApply};
use spirit::schema::signal::{
    ApplyRefusalReason, AuthorizedEvidenceHex, AuthorizedRecordApplication, Certainty, Description,
    Domains, Entry, Importance, Kind, Magnitude, Privacy, RecordIdentifier, Referent, Referents,
    VersionedEntryHex,
};
use tempfile::TempDir;

fn open_store(directory: &TempDir, file: &str) -> Store {
    Store::open(directory.path().join(file)).expect("open sema store")
}

fn intent_entry(description: &str) -> Entry {
    Entry {
        domains: Domains::from_strings(vec![String::from("Information/Documentation")]),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: Certainty::new(Magnitude::High),
        importance: Importance::new(Magnitude::Medium),
        privacy: Privacy::new(Magnitude::Zero),
        referents: Referents::new(vec![Referent::new("spirit")]),
    }
}

/// Produce a genuine content-addressed head entry the way an originating node
/// would: record the intent into a source store and read back the versioned log
/// head — the exact `VersionedCommitLogEntry` the router relays.
fn originated_entry(record_identifier: &str, description: &str) -> VersionedCommitLogEntry {
    let directory = TempDir::new().expect("source store dir");
    let source = open_store(&directory, "source.sema");
    source
        .import_record(record_identifier.to_owned(), intent_entry(description))
        .expect("record the source intent");
    source
        .versioned_log()
        .expect("read source versioned log")
        .last()
        .cloned()
        .expect("source head entry")
}

fn evidence_bound_to(digest_bytes: &[u8]) -> Evidence {
    Evidence::new(
        ComponentKind::Spirit,
        OperationDigest::from_bytes(digest_bytes),
        AttestedMoment::new(
            AttestedMomentProposition::new(
                TimeWindow {
                    opens_at: TimestampNanos::new(0),
                    closes_at: TimestampNanos::new(u64::MAX),
                },
                RequiredSignatureThreshold::new(0),
                Vec::new(),
            ),
            Vec::new(),
        ),
        Vec::new(),
        Vec::new(),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn rkyv_hex_entry(entry: &VersionedCommitLogEntry) -> String {
    hex(rkyv::to_bytes::<rkyv::rancor::Error>(entry)
        .expect("encode versioned entry")
        .as_slice())
}

fn rkyv_hex_evidence(evidence: &Evidence) -> String {
    hex(rkyv::to_bytes::<rkyv::rancor::Error>(evidence)
        .expect("encode evidence")
        .as_slice())
}

fn application(
    record_identifier: &str,
    entry: &VersionedCommitLogEntry,
    evidence: &Evidence,
) -> AuthorizedRecordApplication {
    AuthorizedRecordApplication {
        record_identifier: RecordIdentifier::new(record_identifier),
        versioned_entry_hex: VersionedEntryHex::new(rkyv_hex_entry(entry)),
        authorized_evidence_hex: AuthorizedEvidenceHex::new(rkyv_hex_evidence(evidence)),
    }
}

#[test]
fn authorized_record_lands_live_and_is_observable() {
    let entry = originated_entry("intent-1", "authorized apply lands");
    let evidence = evidence_bound_to(entry.entry_digest().bytes());
    let request = application("intent-1", &entry, &evidence);

    let prepared = PreparedAuthorizedApply::prepare(&request, None).expect("prepare the apply");

    let directory = TempDir::new().expect("target store dir");
    let target = open_store(&directory, "target.sema");
    // Nothing is present before the authorized verdict.
    assert!(
        target
            .entry_by_identifier("intent-1")
            .expect("read target before apply")
            .is_none()
    );

    let outcome = prepared.resolve(&EvaluationDecision::Authorized, &target);
    let AuthorizedApplyOutcome::Applied(receipt) = outcome else {
        panic!("expected an authorized apply to land, got {outcome:?}");
    };
    assert_eq!(receipt.record_identifier.payload(), "intent-1");

    // A following read returns the record LIVE — no restart, no reload.
    let landed = target
        .entry_by_identifier("intent-1")
        .expect("read target after apply")
        .expect("record is live after authorized apply");
    assert_eq!(landed.description.payload(), "authorized apply lands");
}

#[test]
fn non_authorized_verdict_is_refused_fail_closed() {
    let entry = originated_entry("intent-2", "denied apply never lands");
    let evidence = evidence_bound_to(entry.entry_digest().bytes());
    let request = application("intent-2", &entry, &evidence);

    let directory = TempDir::new().expect("target store dir");
    let target = open_store(&directory, "target.sema");

    for decision in [
        EvaluationDecision::Deferred,
        EvaluationDecision::NonJudgement,
    ] {
        let prepared = PreparedAuthorizedApply::prepare(&request, None).expect("prepare the apply");
        let outcome = prepared.resolve(&decision, &target);
        assert_eq!(
            outcome,
            AuthorizedApplyOutcome::Refused(ApplyRefusalReason::AuthorizationDenied),
            "verdict {decision:?} must refuse"
        );
    }

    // The record never entered the live store on a non-authorized verdict.
    assert!(
        target
            .entry_by_identifier("intent-2")
            .expect("read target after refusal")
            .is_none()
    );
}

#[test]
fn rehash_mismatch_is_refused_fail_closed() {
    let entry = originated_entry("intent-3", "tampered body");
    // Evidence authorizing a DIFFERENT object than the carried entry: a valid
    // Evidence must not be replayable onto another record.
    let foreign_evidence = evidence_bound_to(b"a-different-authorized-object");
    let request = application("intent-3", &entry, &foreign_evidence);

    let refusal = PreparedAuthorizedApply::prepare(&request, None)
        .expect_err("a re-hash mismatch must refuse at prepare");
    assert_eq!(refusal, ApplyRefusalReason::RehashMismatch);
}

#[test]
fn malformed_body_is_refused_fail_closed() {
    let entry = originated_entry("intent-4", "malformed carriage");
    let evidence = evidence_bound_to(entry.entry_digest().bytes());
    let mut request = application("intent-4", &entry, &evidence);
    // Corrupt the carried octets: a non-hex body cannot decode to a record.
    request.versioned_entry_hex = VersionedEntryHex::new("not-valid-hex-octets");

    let refusal = PreparedAuthorizedApply::prepare(&request, None)
        .expect_err("a malformed body must refuse at prepare");
    assert_eq!(refusal, ApplyRefusalReason::MalformedRecord);
}

#[test]
fn identifier_disagreement_is_refused_fail_closed() {
    let entry = originated_entry("intent-5", "identity binding");
    let evidence = evidence_bound_to(entry.entry_digest().bytes());
    // The named identifier disagrees with the record inside the carried entry.
    let request = application("intent-mismatch", &entry, &evidence);

    let refusal = PreparedAuthorizedApply::prepare(&request, None)
        .expect_err("an identifier disagreement must refuse at prepare");
    assert_eq!(refusal, ApplyRefusalReason::MalformedRecord);
}
