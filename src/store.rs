use std::{
    fmt,
    path::{Path, PathBuf},
};

use sema_engine::{
    Engine as SemaDatabase, EngineOpen, IdentifiedAssertion, IdentifiedMutation,
    IdentifiedQueryPlan, IdentifiedRetraction, IdentifiedTableDescriptor, IdentifiedTableReference,
    RecordIdentifier as EngineRecordIdentifier, SchemaVersion, TableName,
};
use thiserror::Error;

use crate::schema::{
    meta_signal::ArchiveDatabaseTarget,
    sema::{
        self as sema_schema, ActorStartFailure as SemaActorStartFailure,
        ActorStopFailure as SemaActorStopFailure, ReadInput as SemaReadInput,
        ReadOutput as SemaReadOutput, SemaEngine, WriteInput as SemaWriteInput,
        WriteOutput as SemaWriteOutput,
    },
    signal::{
        ArchivedRecord, CertaintyChange, CertaintyChangeReceipt, CountedRecords, DatabaseMarker,
        Entry, ErrorReport, FoundRecord, Magnitude, ObservedRecords, Privacy, PrivacySelection,
        Query, RemovalCandidateCollection, RemovalCandidatesCollection, RemoveReceipt, SemaReceipt,
        SkippedRemovalCandidate,
    },
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, TraceEvent, TraceLog, schema::sema::SemaObjectName};

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
    archive_target: ArchiveDatabaseTarget,
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
            SemaWriteInput::ChangeCertainty(change) => match self.change_certainty(change) {
                Ok(Some(receipt)) => SemaWriteOutput::certainty_changed(receipt),
                Ok(None) => SemaWriteOutput::missed(ErrorReport {
                    error_message: String::from("record not found"),
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaWriteOutput::missed(ErrorReport {
                    error_message: error.to_string(),
                    database_marker: self.database_marker(),
                }),
            },
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
                Ok(entries) if !entries.is_empty() => SemaReadOutput::observed(ObservedRecords {
                    record_set: entries,
                    database_marker: self.database_marker(),
                }),
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
                match self.entry_by_identifier(record_identifier) {
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
            archive_target: ArchiveDatabaseTarget::Default,
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

    /// Store the owner-configured archive target (the owner-only meta
    /// `Configure` effect).
    ///
    /// The archive is a SEPARATE database from the live intent log. This
    /// method records WHERE that separate archive database lives; it does NOT
    /// open, move, or touch the live database in any way. Every subsequent
    /// `Record` / `ChangeCertainty` / `Remove` / `Observe` keeps landing on the
    /// same live `*.sema` file the store was opened with. The configured target
    /// is consumed only by `collect_removal_candidates`, which opens the
    /// separate archive database on demand.
    ///
    /// This is owner-config storage, not a database operation, so it is
    /// infallible — there is no sema-engine open at configure time.
    pub fn set_archive_target(&mut self, archive_target: ArchiveDatabaseTarget) {
        self.archive_target = archive_target;
    }

    /// The owner-configured archive target: WHERE the separate archive database
    /// lives. Defaults to [`ArchiveDatabaseTarget::Default`] until an owner
    /// `Configure` sets it.
    pub fn archive_target(&self) -> &ArchiveDatabaseTarget {
        &self.archive_target
    }

    /// Resolve the configured archive target to a concrete `*.sema` path for
    /// the SEPARATE archive database.
    ///
    /// `Default` derives a sibling of the live database file
    /// (`<live-stem>.archive.sema`); `Path` uses the owner-supplied path
    /// verbatim. Either way the resolved path is distinct from the live
    /// database path so the archive never collides with the live log.
    fn archive_database_path(&self) -> PathBuf {
        match &self.archive_target {
            ArchiveDatabaseTarget::Default => {
                let stem = self
                    .path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| String::from("spirit"));
                self.path.with_file_name(format!("{stem}.archive.sema"))
            }
            ArchiveDatabaseTarget::Path(archive_path) => PathBuf::from(archive_path.payload()),
        }
    }

    /// Collect removal-candidate records, archive them into the SEPARATE
    /// archive database at the owner-configured target, and remove them from
    /// the live log.
    ///
    /// This is the peer-callable working operation. For every record matching
    /// the candidate query it asserts a copy into the separate archive database
    /// (opened on demand at the configured target — never the live database),
    /// then retracts the original from the live log. A record that fails to
    /// archive is left in the live log and reported as a
    /// [`SkippedRemovalCandidate`] with [`RemovalCandidateSkipReason::ArchiveFailed`];
    /// a record that vanishes between the match and the retraction is reported
    /// with [`RemovalCandidateSkipReason::RecordAlreadyRemoved`]. The reply
    /// carries the archived records, the removed identifiers, the skipped
    /// candidates, and the live database's post-removal marker.
    pub fn collect_removal_candidates(
        &self,
        collection: RemovalCandidateCollection,
    ) -> Result<RemovalCandidatesCollection, StoreError> {
        let query = collection.into_payload();
        let mut archive = self.open_archive_database()?;
        let mut archived_records = Vec::new();
        let mut removed_identifiers = Vec::new();
        let mut skipped_candidates = Vec::new();
        for record in self.records()? {
            let identifier = record.identifier().value();
            let entry = record.into_value();
            if !entry.matches(&query) {
                continue;
            }
            match archive.archive_entry(entry.clone()) {
                Ok(()) => match self.remove(identifier)? {
                    true => {
                        archived_records.push(ArchivedRecord {
                            record_identifier: identifier,
                            entry,
                        });
                        removed_identifiers.push(identifier);
                    }
                    false => skipped_candidates.push(SkippedRemovalCandidate {
                        record_identifier: identifier,
                        removal_candidate_skip_reason:
                            crate::schema::signal::RemovalCandidateSkipReason::RecordAlreadyRemoved,
                    }),
                },
                Err(_error) => skipped_candidates.push(SkippedRemovalCandidate {
                    record_identifier: identifier,
                    removal_candidate_skip_reason:
                        crate::schema::signal::RemovalCandidateSkipReason::ArchiveFailed,
                }),
            }
        }
        Ok(RemovalCandidatesCollection {
            archived_records,
            removed_identifiers,
            skipped_removal_candidates: skipped_candidates,
            database_marker: self.database_marker(),
        })
    }

    /// Open the SEPARATE archive database at the owner-configured target. This
    /// is a distinct `sema-engine` handle over a distinct `*.sema` file; it is
    /// never the live database handle.
    fn open_archive_database(&self) -> Result<ArchiveDatabase, StoreError> {
        ArchiveDatabase::open(self.archive_database_path())
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

    pub fn entry_by_identifier(&self, identifier: u64) -> Result<Option<Entry>, StoreError> {
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

    fn change_certainty(
        &self,
        change: CertaintyChange,
    ) -> Result<Option<CertaintyChangeReceipt>, StoreError> {
        let record_identifier = change.record_identifier;
        let Some(mut entry) = self.entry_by_identifier(record_identifier)? else {
            return Ok(None);
        };
        entry.magnitude = change.certainty.clone();
        self.database.mutate_identified(IdentifiedMutation::new(
            self.entries,
            EngineRecordIdentifier::new(record_identifier),
            entry,
        ))?;
        Ok(Some(CertaintyChangeReceipt {
            record_identifier,
            certainty: change.certainty,
            database_marker: self.database_marker(),
        }))
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

/// The SEPARATE archive database: a sema-engine identified table over its own
/// `*.sema` file, distinct from the live intent log.
///
/// `CollectRemovalCandidates` opens one of these on demand at the
/// owner-configured [`ArchiveDatabaseTarget`], asserts each removal-candidate
/// `Entry` into it, and drops the handle when the collection completes. The
/// archive owns no relationship to the live `Store` database beyond holding the
/// records the live log let go.
struct ArchiveDatabase {
    database: SemaDatabase,
    entries: IdentifiedTableReference<Entry>,
}

impl ArchiveDatabase {
    fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let mut database = SemaDatabase::open(EngineOpen::new(path.into(), SPIRIT_SCHEMA_VERSION))?;
        let entries =
            database.register_identified_table(IdentifiedTableDescriptor::new(ENTRIES_TABLE))?;
        Ok(Self { database, entries })
    }

    /// Durably assert an archived copy of one removal-candidate `Entry` into the
    /// separate archive database. The archive allocates its own identifier; the
    /// original live identifier travels in the `CollectRemovalCandidates` reply,
    /// not in the archive's identifier space.
    fn archive_entry(&mut self, entry: Entry) -> Result<(), StoreError> {
        self.database
            .assert_identified(IdentifiedAssertion::new(self.entries, entry))?;
        Ok(())
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
