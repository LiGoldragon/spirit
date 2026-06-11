use std::path::PathBuf;

use sema_engine::{
    Assertion, Engine as SemaDatabase, EngineOpen, EngineRecord, QueryPlan, RecordKey,
    SchemaVersion, TableDescriptor, TableName, TableReference,
};

use crate::{
    schema::{
        nexus::{GuardianVerdict, Reject},
        signal::{
            Clarification, DatabaseMarker, Entry, Explanation, GuardianRejectionReason, RecordSet,
            Retirement, Supersession,
        },
    },
    store::StoreError,
};

const GUARDIAN_JOURNAL_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(1);
const GUARDIAN_DECISIONS_TABLE: TableName = TableName::new("guardian-decisions");

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuardianOperation {
    Record(Entry),
    Propose(Entry),
    Clarify(Clarification),
    Supersede(Supersession),
    Retire(Retirement),
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuardianDecision {
    operation: GuardianOperation,
    record_set: RecordSet,
    verdict: GuardianVerdict,
    database_marker: DatabaseMarker,
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
    pub(crate) fn record(entry: Entry) -> Self {
        Self::Record(entry)
    }

    pub(crate) fn propose(entry: Entry) -> Self {
        Self::Propose(entry)
    }

    pub(crate) fn clarify(clarification: Clarification) -> Self {
        Self::Clarify(clarification)
    }

    pub(crate) fn supersede(supersession: Supersession) -> Self {
        Self::Supersede(supersession)
    }

    pub(crate) fn retire(retirement: Retirement) -> Self {
        Self::Retire(retirement)
    }

    pub(crate) fn candidate_entry(&self) -> Option<&Entry> {
        match self {
            Self::Record(entry) | Self::Propose(entry) => Some(entry),
            Self::Supersede(supersession) => Some(&supersession.replacement),
            Self::Clarify(_) | Self::Retire(_) => None,
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Record(_) => "Record",
            Self::Propose(_) => "Propose",
            Self::Clarify(_) => "Clarify",
            Self::Supersede(_) => "Supersede",
            Self::Retire(_) => "Retire",
        }
    }
}

impl GuardianDecision {
    pub(crate) fn new(
        operation: GuardianOperation,
        record_set: RecordSet,
        verdict: GuardianVerdict,
        database_marker: DatabaseMarker,
    ) -> Self {
        Self {
            operation,
            record_set,
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
        let decisions = database.register_table(TableDescriptor::new(GUARDIAN_DECISIONS_TABLE))?;
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
