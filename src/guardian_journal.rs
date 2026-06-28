use std::path::PathBuf;

use sema_engine::{
    Assertion, Engine as SemaDatabase, EngineOpen, EngineRecord, FamilyName, QueryPlan, RecordKey,
    SchemaHash, SchemaVersion, TableDescriptor, TableName, TableReference,
};

#[cfg(feature = "mirror-shipper")]
use crate::schema::signal::Input;
use crate::{
    schema::{
        nexus::{GuardianVerdict, ReferentGuardianVerdict, Reject, RejectReferent},
        signal::{
            Clarification, ClarificationResolution, DatabaseMarker, Entry, Explanation,
            GuardianRejectionReason, Proposal, RecordChange, RecordRequest, RecordSet,
            ReferentGuardianRejectionReason, ReferentRegistration, RegisteredReferents, Retirement,
            Supersession,
        },
    },
    store::StoreError,
};

// Bumped when a sema-engine storage-layout break makes an older journal file
// unreadable by the current engine. The journal filename carries the same
// version (see `Store::guardian_journal_path`), so a new daemon opens a fresh
// file instead of failing on an incompatible layout; older files stay on disk
// untouched, readable by the matching previous engine.
const GUARDIAN_JOURNAL_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(5);
const GUARDIAN_DECISIONS_TABLE: TableName = TableName::new("guardian-decisions");
// The journal is hand-written audit state, not a schema-declared wire family,
// so its family identity hashes the journal version label by hand. The label
// moves with GUARDIAN_JOURNAL_SCHEMA_VERSION.
const GUARDIAN_DECISIONS_FAMILY_LABEL: &str = "spirit:guardian-journal:v5";

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuardianOperation {
    Record(RecordRequest),
    Propose(Proposal),
    Clarify(Clarification),
    ResolveClarification(ClarificationResolution),
    Supersede(Supersession),
    Retire(Retirement),
    ChangeRecord(RecordChange),
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuardianDecision {
    Record {
        operation: GuardianOperation,
        record_set: RecordSet,
        verdict: GuardianVerdict,
        database_marker: DatabaseMarker,
    },
    Referent {
        registration: ReferentRegistration,
        registered_referents: RegisteredReferents,
        verdict: ReferentGuardianVerdict,
        database_marker: DatabaseMarker,
    },
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct GuardianJournalEntry {
    decision_identifier: String,
    decision: GuardianDecision,
}

pub(crate) struct GuardianJournal {
    database: SemaDatabase,
    decisions: TableReference<GuardianJournalEntry>,
}

impl GuardianOperation {
    pub(crate) fn record(request: RecordRequest) -> Self {
        Self::Record(request)
    }

    pub(crate) fn propose(proposal: Proposal) -> Self {
        Self::Propose(proposal)
    }

    pub(crate) fn clarify(clarification: Clarification) -> Self {
        Self::Clarify(clarification)
    }

    pub(crate) fn resolve_clarification(resolution: ClarificationResolution) -> Self {
        Self::ResolveClarification(resolution)
    }

    pub(crate) fn supersede(supersession: Supersession) -> Self {
        Self::Supersede(supersession)
    }

    pub(crate) fn retire(retirement: Retirement) -> Self {
        Self::Retire(retirement)
    }

    pub(crate) fn change_record(change: RecordChange) -> Self {
        Self::ChangeRecord(change)
    }

    /// Whether the operation's justification carries no verbatim testimony at
    /// all. Empty testimony is a structural fact the daemon checks
    /// deterministically — a candidate with zero quotes has produced no evidence
    /// — rather than relying on the model to notice an empty vector.
    pub(crate) fn testimony_is_empty(&self) -> bool {
        let justification = match self {
            Self::Record(request) => &request.justification,
            Self::Propose(proposal) => &proposal.justification,
            Self::Clarify(clarification) => &clarification.justification,
            Self::ResolveClarification(resolution) => &resolution.justification,
            Self::Supersede(supersession) => &supersession.justification,
            Self::Retire(retirement) => &retirement.justification,
            Self::ChangeRecord(change) => &change.justification,
        };
        justification.testimony.payload().is_empty()
    }

    pub(crate) fn candidate_entries(&self) -> Vec<&Entry> {
        match self {
            Self::Record(request) => vec![&request.entry],
            Self::Propose(proposal) => vec![&proposal.entry],
            Self::Supersede(supersession) => supersession.replacements.payload().iter().collect(),
            Self::ChangeRecord(change) => vec![&change.entry],
            Self::Clarify(_) | Self::ResolveClarification(_) | Self::Retire(_) => Vec::new(),
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Record(_) => "Record",
            Self::Propose(_) => "Propose",
            Self::Clarify(_) => "Clarify",
            Self::ResolveClarification(_) => "ResolveClarification",
            Self::Supersede(_) => "Supersede",
            Self::Retire(_) => "Retire",
            Self::ChangeRecord(_) => "ChangeRecord",
        }
    }

    #[cfg(feature = "mirror-shipper")]
    pub(crate) fn authorization_context(
        &self,
        target_key: signal_criome::SpiritProcessKey,
    ) -> signal_criome::SpiritAuthorizationContext {
        signal_criome::SpiritAuthorizationContext {
            operation_name: signal_criome::SpiritOperationName::new(self.name()),
            raw_payload: signal_criome::RawSpiritOperationPayload::new(nota::NotaEncode::to_nota(
                &self.as_signal_input(),
            )),
            target_key,
        }
    }

    #[cfg(feature = "mirror-shipper")]
    fn as_signal_input(&self) -> Input {
        match self {
            Self::Record(request) => Input::record(request.clone()),
            Self::Propose(proposal) => Input::propose(proposal.clone()),
            Self::Clarify(clarification) => Input::clarify(clarification.clone()),
            Self::ResolveClarification(resolution) => {
                Input::resolve_clarification(resolution.clone())
            }
            Self::Supersede(supersession) => Input::supersede(supersession.clone()),
            Self::Retire(retirement) => Input::retire(retirement.clone()),
            Self::ChangeRecord(change) => Input::change_record(change.clone()),
        }
    }
}

impl GuardianDecision {
    pub(crate) fn record(
        operation: GuardianOperation,
        record_set: RecordSet,
        verdict: GuardianVerdict,
        database_marker: DatabaseMarker,
    ) -> Self {
        Self::Record {
            operation,
            record_set,
            verdict,
            database_marker,
        }
    }

    pub(crate) fn referent(
        registration: ReferentRegistration,
        registered_referents: RegisteredReferents,
        verdict: ReferentGuardianVerdict,
        database_marker: DatabaseMarker,
    ) -> Self {
        Self::Referent {
            registration,
            registered_referents,
            verdict,
            database_marker,
        }
    }
}

impl GuardianVerdict {
    pub(crate) fn from_harness_rejection(
        reason: GuardianRejectionReason,
        explanation: Explanation,
    ) -> Self {
        Self::reject(Reject {
            guardian_rejection_reason: reason,
            explanation,
        })
    }
}

impl ReferentGuardianVerdict {
    pub(crate) fn from_harness_rejection(
        reason: ReferentGuardianRejectionReason,
        explanation: Explanation,
    ) -> Self {
        Self::reject_referent(RejectReferent {
            referent_guardian_rejection_reason: reason,
            explanation,
        })
    }
}

impl GuardianJournalEntry {
    fn new(decision_identifier: String, decision: GuardianDecision) -> Self {
        Self {
            decision_identifier,
            decision,
        }
    }
}

impl EngineRecord for GuardianJournalEntry {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.decision_identifier.clone())
    }
}

impl GuardianJournal {
    pub(crate) fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let mut database = SemaDatabase::open(EngineOpen::new(
            path.into(),
            GUARDIAN_JOURNAL_SCHEMA_VERSION,
        ))?;
        let decisions = database.register_table(TableDescriptor::new(
            GUARDIAN_DECISIONS_TABLE,
            FamilyName::new("GuardianDecisionsFamily"),
            SchemaHash::for_label(GUARDIAN_DECISIONS_FAMILY_LABEL),
        ))?;
        Ok(Self {
            database,
            decisions,
        })
    }

    pub(crate) fn append(&mut self, decision: GuardianDecision) -> Result<(), StoreError> {
        let decision_identifier = self.next_decision_identifier()?;
        self.database.assert(Assertion::new(
            self.decisions,
            GuardianJournalEntry::new(decision_identifier, decision),
        ))?;
        Ok(())
    }

    pub(crate) fn len(&self) -> Result<usize, StoreError> {
        Ok(self
            .database
            .match_records(QueryPlan::all(self.decisions))?
            .records()
            .len())
    }

    fn next_decision_identifier(&self) -> Result<String, StoreError> {
        Ok(format!(
            "guardian-decision-{}",
            self.database.current_commit_sequence()?.value() + 1
        ))
    }
}
