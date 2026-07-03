//! The quorum-gated authorized-apply ingress (Spirit `xhwa`, piece 4).
//!
//! An arriving record lands LIVE into the running store only after the LOCAL
//! criome re-judges the carried Evidence, fail-closed. The router hands a
//! `signal-spirit` `ApplyAuthorizedRecord` frame to the working socket; Spirit
//! decodes the carried rkyv [`VersionedCommitLogEntry`] and signal-criome
//! [`Evidence`], verifies the content-address binding (the carried entry
//! re-hashes to the digest the Evidence authorized), hands the assembled
//! [`AuthorizationEvaluation`] to the co-resident criome, and imports via
//! [`Store::import_record`] only on [`EvaluationDecision::Authorized`].
//!
//! The re-judge is NOT owner-trust: unlike the owner-only meta `Import`, this
//! ingress applies a foreign record solely because the quorum authorized it,
//! confirmed independently by THIS node's criome from the carried Evidence.
//! `import_record` writes the same live `Arc<SemaDatabase>` that `Observe` reads,
//! so an immediately following `Observe` returns the applied record with no
//! restart or reload.
//!
//! Parsing/verification ([`PreparedAuthorizedApply::prepare`]) and the
//! decision gate ([`PreparedAuthorizedApply::resolve`]) are split from the
//! criome round-trip so the fail-closed behaviour is exercised against a
//! locally-assembled Evidence without a live criome daemon.

use rkyv::rancor;
use sema_engine::VersionedCommitLogEntry;
use signal_criome::{
    AuthorizationEvaluation, AuthorizedObjectKind, AuthorizedObjectReference, ComponentKind,
    ContractDigest, EvaluationDecision, Evidence, ObjectDigest, OperationDigest,
};

use crate::{
    schema::{
        sema::StoredRecord,
        signal::{
            ApplyRefusal, ApplyRefusalReason, AuthorizedRecordApplication, Entry, Output,
            RecordApplicationReceipt, RecordIdentifier,
        },
    },
    store::Store,
};

/// A hex-encoded rkyv octet blob carried on the wire — the `VersionedEntryHex` /
/// `AuthorizedEvidenceHex` carriage the `signal-spirit` contract uses to relay a
/// binary object through a NOTA-projectable frame (the same shape the meta
/// `HeadObjectHex` uses). It owns the borrowed hex text and decodes it to octets.
struct CarriedHex<'text> {
    text: &'text str,
}

impl<'text> CarriedHex<'text> {
    fn new(text: &'text str) -> Self {
        Self { text }
    }

    /// Decode the even-length lowercase-or-uppercase hex text to its octets. A
    /// malformed body (odd length or a non-hex digit) is a malformed record, not
    /// a panic.
    fn octets(&self) -> Result<Vec<u8>, ApplyRefusalReason> {
        if self.text.len() % 2 != 0 {
            return Err(ApplyRefusalReason::MalformedRecord);
        }
        (0..self.text.len())
            .step_by(2)
            .map(|start| {
                u8::from_str_radix(&self.text[start..start + 2], 16)
                    .map_err(|_| ApplyRefusalReason::MalformedRecord)
            })
            .collect()
    }
}

/// A parsed, content-verified authorized-apply request ready for the criome
/// verdict: the assembled [`AuthorizationEvaluation`] to re-judge, plus the
/// `(record_identifier, entry)` extracted from the carried versioned entry that
/// lands on [`EvaluationDecision::Authorized`].
#[derive(Clone, Debug)]
pub struct PreparedAuthorizedApply {
    evaluation: AuthorizationEvaluation,
    record_identifier: String,
    entry: Entry,
}

impl PreparedAuthorizedApply {
    /// Parse the arriving request and verify the content-address binding.
    ///
    /// Decodes the carried [`VersionedCommitLogEntry`] and [`Evidence`], and
    /// refuses fail-closed when: a body is malformed
    /// ([`ApplyRefusalReason::MalformedRecord`]); the carried record's identity
    /// disagrees with the named `record_identifier` (`MalformedRecord`); or the
    /// carried entry does not re-hash to the operation digest the Evidence
    /// authorized ([`ApplyRefusalReason::RehashMismatch`]) — the check that
    /// stops a valid Evidence from being replayed onto a different record.
    ///
    /// `contract` is the admitted mirror-contract digest the local criome will
    /// judge against; when absent it falls back to the object-derived digest,
    /// mirroring the origination attestor's unsigned bootstrap.
    pub fn prepare(
        request: &AuthorizedRecordApplication,
        contract: Option<ContractDigest>,
    ) -> Result<Self, ApplyRefusalReason> {
        let entry_octets =
            CarriedHex::new(request.versioned_entry_hex.payload().as_str()).octets()?;
        let versioned_entry =
            rkyv::from_bytes::<VersionedCommitLogEntry, rancor::Error>(&entry_octets)
                .map_err(|_| ApplyRefusalReason::MalformedRecord)?;
        let evidence_octets =
            CarriedHex::new(request.authorized_evidence_hex.payload().as_str()).octets()?;
        let evidence = rkyv::from_bytes::<Evidence, rancor::Error>(&evidence_octets)
            .map_err(|_| ApplyRefusalReason::MalformedRecord)?;

        let entry_digest = versioned_entry.entry_digest();
        // The content-address binding: the Evidence's authorized operation digest
        // must be the digest of THIS carried entry. A tampered body re-hashes to a
        // different digest and is refused before any verdict is sought.
        if evidence.operation != OperationDigest::from_bytes(entry_digest.bytes()) {
            return Err(ApplyRefusalReason::RehashMismatch);
        }

        let stored = Self::stored_record(&versioned_entry)?;
        if stored.record_identifier.payload() != request.record_identifier.payload() {
            return Err(ApplyRefusalReason::MalformedRecord);
        }

        let object = AuthorizedObjectReference {
            component: ComponentKind::Spirit,
            digest: ObjectDigest::from_bytes(entry_digest.bytes()),
            kind: AuthorizedObjectKind::Head,
        };
        let contract = contract.unwrap_or_else(|| {
            ContractDigest::from(ObjectDigest::from_bytes(entry_digest.bytes()))
        });
        Ok(Self {
            evaluation: AuthorizationEvaluation {
                contract,
                object,
                evidence,
            },
            record_identifier: stored.record_identifier.into_payload(),
            entry: stored.entry,
        })
    }

    /// Decode the record the carried entry commits: the head operation's rkyv
    /// [`StoredRecord`] payload. A tombstone or an unparsable payload is a
    /// malformed record.
    fn stored_record(
        versioned_entry: &VersionedCommitLogEntry,
    ) -> Result<StoredRecord, ApplyRefusalReason> {
        let payload_octets = versioned_entry
            .operations()
            .head()
            .payload()
            .bytes()
            .ok_or(ApplyRefusalReason::MalformedRecord)?;
        rkyv::from_bytes::<StoredRecord, rancor::Error>(payload_octets)
            .map_err(|_| ApplyRefusalReason::MalformedRecord)
    }

    /// The assembled evaluation the local criome re-judges before the apply.
    pub fn evaluation(&self) -> &AuthorizationEvaluation {
        &self.evaluation
    }

    /// Apply the fail-closed gate: land the record into the live store on
    /// [`EvaluationDecision::Authorized`], refuse on any other verdict.
    ///
    /// Only `Authorized` reaches [`Store::import_record`]; a store fault at the
    /// append is reported as [`ApplyRefusalReason::MalformedRecord`] (the parsed
    /// record could not land), never a silent success.
    pub fn resolve(self, decision: &EvaluationDecision, store: &Store) -> AuthorizedApplyOutcome {
        match decision {
            EvaluationDecision::Authorized => {
                let Self {
                    record_identifier,
                    entry,
                    ..
                } = self;
                match store.import_record(record_identifier, entry) {
                    Ok(record_identifier) => {
                        AuthorizedApplyOutcome::Applied(RecordApplicationReceipt {
                            record_identifier: RecordIdentifier::new(record_identifier),
                            database_marker: store.database_marker(),
                        })
                    }
                    Err(_error) => {
                        AuthorizedApplyOutcome::Refused(ApplyRefusalReason::MalformedRecord)
                    }
                }
            }
            EvaluationDecision::Rejected(_)
            | EvaluationDecision::Deferred
            | EvaluationDecision::NonJudgement
            | EvaluationDecision::Escalate(_) => {
                AuthorizedApplyOutcome::Refused(ApplyRefusalReason::AuthorizationDenied)
            }
        }
    }
}

/// The terminal outcome of an authorized-apply ingress: the record landed with
/// its receipt, or a typed refusal. Projects to the `signal-spirit` reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorizedApplyOutcome {
    Applied(RecordApplicationReceipt),
    Refused(ApplyRefusalReason),
}

impl AuthorizedApplyOutcome {
    pub fn into_output(self) -> Output {
        match self {
            Self::Applied(receipt) => Output::record_applied(receipt),
            Self::Refused(reason) => Output::apply_refused(ApplyRefusal::new(reason)),
        }
    }
}
