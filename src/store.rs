use std::{
    fmt,
    path::{Path, PathBuf},
};

use sema_engine::{
    Engine as SemaDatabase, EngineOpen, IdentifiedAssertion, IdentifiedQueryPlan,
    IdentifiedRetraction, IdentifiedTableDescriptor, IdentifiedTableReference,
    RecordIdentifier as EngineRecordIdentifier, SchemaVersion, TableName,
};
use thiserror::Error;

use crate::{
    CountedRecords, DatabaseMarker, Entry, ErrorReport, FoundRecord, Magnitude, Privacy,
    PrivacySelection, Query, RemoveReceipt, SemaActorStartFailure, SemaActorStopFailure,
    SemaEngine, SemaReadInput, SemaReadOutput, SemaReceipt, SemaWriteInput, SemaWriteOutput,
    schema::sema as sema_schema,
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, SemaObjectName, TraceEvent, TraceLog};

const SPIRIT_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(1);
const ENTRIES_TABLE: TableName = TableName::new("records");

/// The SEMA durable store: a sema-engine identified table written to a
/// `*.sema` file.
///
/// SEMA means database work. `Store` maps generated SEMA roots onto
/// sema-engine operations; sema-engine owns the database handle, numeric
/// identifier allocation, durable commit sequence, and typed rkyv table
/// access. Query predicate semantics stay here because they are
/// Spirit-specific SEMA behavior, not generic daemon plumbing.
pub struct Store {
    database: SemaDatabase,
    entries: IdentifiedTableReference<Entry>,
    path: PathBuf,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
}

impl fmt::Debug for Store {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Store")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SemaEngine for Store {
    fn on_start(&mut self) -> Result<(), SemaActorStartFailure> {
        #[cfg(feature = "testing-trace")]
        self.trace_sema_activation(SemaObjectName::Started);
        Ok(())
    }

    fn on_stop(&mut self) -> Result<(), SemaActorStopFailure> {
        #[cfg(feature = "testing-trace")]
        self.trace_sema_activation(SemaObjectName::Stopped);
        Ok(())
    }

    #[cfg(feature = "testing-trace")]
    fn trace_sema_activation(&self, object_name: SemaObjectName) {
        self.trace_log
            .record(TraceEvent::new(ObjectName::Sema(object_name)));
    }

    fn apply_inner(
        &mut self,
        command: sema_schema::sema::Sema<sema_schema::sema::WriteInput>,
    ) -> sema_schema::sema::Sema<sema_schema::sema::WriteOutput> {
        let origin_route = command.origin_route();
        let output = match command.into_root() {
            SemaWriteInput::Record(record) => match self.record(record) {
                Ok(identifier) => SemaWriteOutput::recorded(SemaReceipt {
                    record_identifier: identifier,
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaWriteOutput::missed(ErrorReport {
                    error_message: error.to_string(),
                    database_marker: self.database_marker(),
                }),
            },
            SemaWriteInput::Remove(remove) => {
                let record_identifier = remove;
                match self.remove(record_identifier) {
                    Ok(true) => SemaWriteOutput::removed(RemoveReceipt {
                        record_identifier,
                        database_marker: self.database_marker(),
                    }),
                    Ok(false) => SemaWriteOutput::missed(ErrorReport {
                        error_message: String::from("record not found"),
                        database_marker: self.database_marker(),
                    }),
                    Err(error) => SemaWriteOutput::missed(ErrorReport {
                        error_message: error.to_string(),
                        database_marker: self.database_marker(),
                    }),
                }
            }
        };
        output.with_origin_route(origin_route)
    }

    fn observe_inner(
        &self,
        query: sema_schema::sema::Sema<sema_schema::sema::ReadInput>,
    ) -> sema_schema::sema::Sema<sema_schema::sema::ReadOutput> {
        let origin_route = query.origin_route();
        let output = match query.into_root() {
            SemaReadInput::Observe(observe) => match self.observe(&observe) {
                Ok(entries) if !entries.is_empty() => {
                    SemaReadOutput::observed(crate::ObservedRecords {
                        record_set: entries,
                        database_marker: self.database_marker(),
                    })
                }
                Ok(_) => SemaReadOutput::missed(ErrorReport {
                    error_message: String::from("no matching record"),
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaReadOutput::missed(ErrorReport {
                    error_message: error.to_string(),
                    database_marker: self.database_marker(),
                }),
            },
            SemaReadInput::Lookup(lookup) => {
                let record_identifier = lookup;
                match self.lookup(record_identifier) {
                    Ok(Some(entry)) => SemaReadOutput::found(FoundRecord {
                        record_identifier,
                        entry,
                        database_marker: self.database_marker(),
                    }),
                    Ok(None) => SemaReadOutput::missed(ErrorReport {
                        error_message: String::from("record not found"),
                        database_marker: self.database_marker(),
                    }),
                    Err(error) => SemaReadOutput::missed(ErrorReport {
                        error_message: error.to_string(),
                        database_marker: self.database_marker(),
                    }),
                }
            }
            SemaReadInput::Count(count) => match self.count(&count) {
                Ok(count) => SemaReadOutput::counted(CountedRecords {
                    record_count: count,
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaReadOutput::missed(ErrorReport {
                    error_message: error.to_string(),
                    database_marker: self.database_marker(),
                }),
            },
        };
        output.with_origin_route(origin_route)
    }
}

impl Store {
    /// Open or create the durable SEMA database at `path`.
    ///
    /// A fresh file is created with empty engine counters; an existing
    /// file resumes its persisted commit sequence and record identifier
    /// counter through sema-engine.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let mut database =
            SemaDatabase::open(EngineOpen::new(path.clone(), SPIRIT_SCHEMA_VERSION))?;
        let entries =
            database.register_identified_table(IdentifiedTableDescriptor::new(ENTRIES_TABLE))?;
        Ok(Self {
            database,
            entries,
            path,
            #[cfg(feature = "testing-trace")]
            trace_log: TraceLog::default(),
        })
    }

    #[cfg(feature = "testing-trace")]
    pub fn open_with_trace(
        path: impl Into<PathBuf>,
        trace_log: TraceLog,
    ) -> Result<Self, StoreError> {
        Self::open(path).map(|store| store.with_trace(trace_log))
    }

    #[cfg(feature = "testing-trace")]
    pub fn with_trace(mut self, trace_log: TraceLog) -> Self {
        self.trace_log = trace_log;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn record(&self, entry: Entry) -> Result<u64, StoreError> {
        Ok(self
            .database
            .assert_identified(IdentifiedAssertion::new(self.entries, entry))?
            .identifier()
            .value())
    }

    fn observe(&self, query: &Query) -> Result<Vec<Entry>, StoreError> {
        Ok(self
            .records()?
            .into_iter()
            .map(|record| record.into_value())
            .filter(|entry| entry.matches(query))
            .collect())
    }

    fn lookup(&self, identifier: u64) -> Result<Option<Entry>, StoreError> {
        Ok(self
            .database
            .match_identified(IdentifiedQueryPlan::identifier(
                self.entries,
                EngineRecordIdentifier::new(identifier),
            ))?
            .into_records()
            .into_iter()
            .next()
            .map(|record| record.into_value()))
    }

    fn count(&self, query: &Query) -> Result<u64, StoreError> {
        Ok(self.observe(query)?.len() as u64)
    }

    fn remove(&self, identifier: u64) -> Result<bool, StoreError> {
        match self.database.retract_identified(IdentifiedRetraction::new(
            self.entries,
            EngineRecordIdentifier::new(identifier),
        )) {
            Ok(_receipt) => Ok(true),
            Err(sema_engine::Error::RecordNotFound { .. }) => Ok(false),
            Err(error) => Err(StoreError::Database(error)),
        }
    }

    pub fn len(&self) -> usize {
        self.committed_record_count().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn committed_record_count(&self) -> Result<usize, StoreError> {
        Ok(self.records()?.len())
    }

    /// The SEMA commit marker: the persisted commit sequence plus a real
    /// content hash of the committed records.
    pub fn database_marker(&self) -> DatabaseMarker {
        DatabaseMarker {
            commit_sequence: self.commit_sequence().unwrap_or(0),
            state_digest: self.state_digest().unwrap_or(0),
        }
    }

    fn commit_sequence(&self) -> Result<u64, StoreError> {
        Ok(self.database.current_commit_sequence()?.value())
    }

    /// A content-addressed digest of committed state: blake3 over each
    /// record's `(identifier, archived bytes)`, folded with the commit
    /// sequence, reduced to the schema's `Integer` digest width. An empty
    /// store (no committed records) digests to zero, so a marker taken
    /// before any write reads `(0, 0)`.
    fn state_digest(&self) -> Result<u64, StoreError> {
        let records = self.records()?;
        if records.is_empty() {
            return Ok(0);
        }
        let commit_sequence = self.commit_sequence()?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(&commit_sequence.to_le_bytes());
        for record in records {
            let archive = rkyv::to_bytes::<rkyv::rancor::Error>(record.value())
                .map_err(|_| StoreError::ArchiveEncode)?;
            hasher.update(&record.identifier().value().to_le_bytes());
            hasher.update(&archive);
        }
        let digest = hasher.finalize();
        let mut head = [0_u8; 8];
        head.copy_from_slice(&digest.as_bytes()[..8]);
        Ok(u64::from_le_bytes(head))
    }

    fn records(&self) -> Result<Vec<sema_engine::IdentifiedRecord<Entry>>, StoreError> {
        Ok(self
            .database
            .match_identified(IdentifiedQueryPlan::all(self.entries))?
            .into_records())
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sema database engine error: {0}")]
    Database(#[from] sema_engine::Error),

    #[error("failed to encode record rkyv archive")]
    ArchiveEncode,
}

impl Entry {
    pub fn matches(&self, query: &Query) -> bool {
        query.matches(self)
    }

    pub fn magnitude_weight(&self) -> u64 {
        self.magnitude.weight()
    }
}

impl Query {
    pub fn matches(&self, entry: &Entry) -> bool {
        self.topic_match.matches(&entry.topics)
            && self.kind.as_ref().is_none_or(|kind| &entry.kind == kind)
            && self.privacy_selection.matches(&entry.privacy)
    }
}

impl PrivacySelection {
    pub fn default_observation_privacy() -> Self {
        Self::exact(Magnitude::Zero)
    }

    pub fn matches(&self, privacy: &Privacy) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(expected) => privacy == expected,
            Self::AtMost(maximum) => privacy.weight() <= maximum.weight(),
            Self::AtLeast(minimum) => privacy.weight() >= minimum.weight(),
        }
    }
}

impl Magnitude {
    pub fn weight(&self) -> u64 {
        match self {
            Self::Zero => 0,
            Self::Minimum => 1,
            Self::VeryLow => 2,
            Self::Low => 3,
            Self::Medium => 4,
            Self::High => 5,
            Self::VeryHigh => 6,
            Self::Maximum => 7,
        }
    }
}
