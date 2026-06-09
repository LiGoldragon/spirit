use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
};

use sema_engine::{
    Assertion, Engine as SemaDatabase, EngineOpen, EngineRecord, Mutation, QueryPlan, RecordKey,
    Retraction, SchemaVersion, TableDescriptor, TableName, TableReference,
};
use thiserror::Error;

use crate::schema::{
    meta_signal::ArchiveDatabaseTarget,
    sema::{
        self as sema_schema, EngineStartFailure as SemaEngineStartFailure,
        EngineStopFailure as SemaEngineStopFailure, ReadInput as SemaReadInput,
        ReadOutput as SemaReadOutput, SemaEngine, WriteInput as SemaWriteInput,
        WriteOutput as SemaWriteOutput,
    },
    signal::{
        ArchivedRecord, CertaintyChange, CertaintyChangeReceipt, CountedRecords, DatabaseMarker,
        Entry, ErrorReport, FoundRecord, Magnitude, ObservedRecords, Privacy, PrivacySelection,
        Query, RecordChange, RecordChangeReceipt, RemovalCandidateCollection,
        RemovalCandidatesCollection, RemoveReceipt, SemaReceipt, SkippedRemovalCandidate,
    },
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, TraceEvent, TraceLog, schema::sema::SemaObjectName};

const SPIRIT_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(1);
const ENTRIES_TABLE: TableName = TableName::new("records");
const RECORD_IDENTIFIER_MINIMUM_CODE_LENGTH: usize = 4;
const RECORD_IDENTIFIER_MAXIMUM_CODE_LENGTH: usize = 7;
const RECORD_IDENTIFIER_CODE_RADIX: u64 = 36;
const RANDOM_IDENTIFIER_ATTEMPTS_PER_LENGTH: usize = 128;

/// The SEMA durable store: a sema-engine keyed table written to a `*.sema`
/// file.
///
/// SEMA means database work. `Store` maps generated SEMA roots onto
/// sema-engine operations; sema-engine owns the database handle, durable
/// commit sequence, and typed rkyv table access. Spirit owns the
/// production-compatible short/base36 record identifiers because migration must
/// preserve them as stable keys. Query predicate semantics stay here because
/// they are Spirit-specific SEMA behavior, not generic daemon plumbing.
pub struct Store {
    database: SemaDatabase,
    entries: TableReference<StoredRecord>,
    path: PathBuf,
    archive_target: ArchiveDatabaseTarget,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct StoredRecord {
    record_identifier: String,
    entry: Entry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordIdentifierMint {
    used_identifiers: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecordIdentifierCodeRange {
    first_value: u64,
    value_count: u64,
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
    fn on_start(&mut self) -> Result<(), SemaEngineStartFailure> {
        #[cfg(feature = "testing-trace")]
        self.trace_sema_activation(SemaObjectName::Started);
        Ok(())
    }

    fn on_stop(&mut self) -> Result<(), SemaEngineStopFailure> {
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
                match self.remove(&record_identifier) {
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
            SemaWriteInput::ChangeRecord(change) => match self.change_record(change) {
                Ok(Some(receipt)) => SemaWriteOutput::record_changed(receipt),
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
                match self.entry_by_identifier(&record_identifier) {
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
        let entries = database.register_table(TableDescriptor::new(ENTRIES_TABLE))?;
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
            let identifier = record.record_identifier.clone();
            if !record.entry.matches(&query) {
                continue;
            }
            match archive.archive_record(record.clone()) {
                Ok(()) => match self.remove(&identifier)? {
                    true => {
                        archived_records.push(ArchivedRecord {
                            record_identifier: identifier.clone(),
                            entry: record.entry,
                        });
                        removed_identifiers.push(identifier);
                    }
                    false => skipped_candidates.push(SkippedRemovalCandidate {
                        record_identifier: identifier.clone(),
                        removal_candidate_skip_reason:
                            crate::schema::signal::RemovalCandidateSkipReason::RecordAlreadyRemoved,
                    }),
                },
                Err(_error) => skipped_candidates.push(SkippedRemovalCandidate {
                    record_identifier: identifier.clone(),
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

    pub fn import_record(
        &self,
        record_identifier: String,
        entry: Entry,
    ) -> Result<String, StoreError> {
        self.database.assert(Assertion::new(
            self.entries,
            StoredRecord::new(record_identifier.clone(), entry),
        ))?;
        Ok(record_identifier)
    }

    fn record(&self, entry: Entry) -> Result<String, StoreError> {
        let record_identifier = self.next_record_identifier()?;
        self.import_record(record_identifier.clone(), entry)?;
        Ok(record_identifier)
    }

    fn observe(&self, query: &Query) -> Result<Vec<Entry>, StoreError> {
        Ok(self
            .records()?
            .into_iter()
            .filter(|record| record.entry.matches(query))
            .map(StoredRecord::into_entry)
            .collect())
    }

    pub fn entry_by_identifier(&self, identifier: &str) -> Result<Option<Entry>, StoreError> {
        Ok(self
            .database
            .match_records(QueryPlan::key(self.entries, RecordKey::new(identifier)))?
            .records()
            .iter()
            .next()
            .map(StoredRecord::entry))
    }

    fn count(&self, query: &Query) -> Result<u64, StoreError> {
        Ok(self.observe(query)?.len() as u64)
    }

    fn remove(&self, identifier: &str) -> Result<bool, StoreError> {
        match self
            .database
            .retract(Retraction::new(self.entries, RecordKey::new(identifier)))
        {
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
        let Some(mut entry) = self.entry_by_identifier(&record_identifier)? else {
            return Ok(None);
        };
        entry.magnitude = change.certainty;
        self.database.mutate(Mutation::new(
            self.entries,
            StoredRecord::new(record_identifier.clone(), entry),
        ))?;
        Ok(Some(CertaintyChangeReceipt {
            record_identifier,
            certainty: change.certainty,
            database_marker: self.database_marker(),
        }))
    }

    fn change_record(
        &self,
        change: RecordChange,
    ) -> Result<Option<RecordChangeReceipt>, StoreError> {
        let record_identifier = change.record_identifier;
        if self.entry_by_identifier(&record_identifier)?.is_none() {
            return Ok(None);
        }
        self.database.mutate(Mutation::new(
            self.entries,
            StoredRecord::new(record_identifier.clone(), change.entry),
        ))?;
        Ok(Some(RecordChangeReceipt {
            record_identifier,
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
            let archive = rkyv::to_bytes::<rkyv::rancor::Error>(&record)
                .map_err(|_| StoreError::ArchiveEncode)?;
            hasher.update(record.record_identifier.as_bytes());
            hasher.update(&archive);
        }
        let digest = hasher.finalize();
        let mut head = [0_u8; 8];
        head.copy_from_slice(&digest.as_bytes()[..8]);
        Ok(u64::from_le_bytes(head))
    }

    fn records(&self) -> Result<Vec<StoredRecord>, StoreError> {
        Ok(self
            .database
            .match_records(QueryPlan::all(self.entries))?
            .records()
            .to_vec())
    }

    fn next_record_identifier(&self) -> Result<String, StoreError> {
        RecordIdentifierMint::from_records(&self.records()?).next_identifier()
    }
}

impl StoredRecord {
    fn new(record_identifier: String, entry: Entry) -> Self {
        Self {
            record_identifier,
            entry,
        }
    }

    fn into_entry(self) -> Entry {
        self.entry
    }

    fn entry(&self) -> Entry {
        self.entry.clone()
    }
}

impl EngineRecord for StoredRecord {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.record_identifier.clone())
    }
}

impl RecordIdentifierMint {
    fn from_records(records: &[StoredRecord]) -> Self {
        Self {
            used_identifiers: records
                .iter()
                .map(|record| record.record_identifier.clone())
                .collect(),
        }
    }

    fn next_identifier(&self) -> Result<String, StoreError> {
        for code_length in
            RECORD_IDENTIFIER_MINIMUM_CODE_LENGTH..=RECORD_IDENTIFIER_MAXIMUM_CODE_LENGTH
        {
            if let Some(identifier) = self.identifier_for_code_length(code_length)? {
                return Ok(identifier);
            }
        }
        Err(StoreError::IdentifierMint(format!(
            "no available record identifier code between {RECORD_IDENTIFIER_MINIMUM_CODE_LENGTH} and {RECORD_IDENTIFIER_MAXIMUM_CODE_LENGTH} characters"
        )))
    }

    fn identifier_for_code_length(&self, code_length: usize) -> Result<Option<String>, StoreError> {
        let range = RecordIdentifierCodeRange::new(code_length);
        for _ in 0..RANDOM_IDENTIFIER_ATTEMPTS_PER_LENGTH {
            let identifier = range.random_identifier()?;
            if !self.used_identifiers.contains(&identifier) {
                return Ok(Some(identifier));
            }
        }
        Ok(range.first_available_identifier(&self.used_identifiers))
    }
}

impl RecordIdentifierCodeRange {
    fn new(code_length: usize) -> Self {
        let first_value = if code_length == RECORD_IDENTIFIER_MINIMUM_CODE_LENGTH {
            0
        } else {
            Self::radix_power(code_length - 1)
        };
        let next_length_first_value = Self::radix_power(code_length);
        Self {
            first_value,
            value_count: next_length_first_value - first_value,
        }
    }

    fn random_identifier(self) -> Result<String, StoreError> {
        let mut bytes = [0_u8; 8];
        getrandom::fill(&mut bytes)
            .map_err(|error| StoreError::IdentifierMint(error.to_string()))?;
        let offset = u64::from_be_bytes(bytes) % self.value_count;
        Ok(Self::code_from_value(self.first_value + offset))
    }

    fn first_available_identifier(self, used_identifiers: &BTreeSet<String>) -> Option<String> {
        let last_value = self.first_value + self.value_count;
        (self.first_value..last_value)
            .map(Self::code_from_value)
            .find(|identifier| !used_identifiers.contains(identifier))
    }

    fn code_from_value(mut value: u64) -> String {
        let mut digits = Vec::new();
        while value > 0 {
            let digit = (value % RECORD_IDENTIFIER_CODE_RADIX) as u8;
            digits.push(Self::digit_character(digit));
            value /= RECORD_IDENTIFIER_CODE_RADIX;
        }
        while digits.len() < RECORD_IDENTIFIER_MINIMUM_CODE_LENGTH {
            digits.push('0');
        }
        digits.iter().rev().collect()
    }

    fn digit_character(digit: u8) -> char {
        match digit {
            0..=9 => char::from(b'0' + digit),
            10..=35 => char::from(b'a' + digit - 10),
            _ => unreachable!("base36 digit is constrained by modulo"),
        }
    }

    fn radix_power(exponent: usize) -> u64 {
        (0..exponent).fold(1, |value, _| value * RECORD_IDENTIFIER_CODE_RADIX)
    }
}

/// The SEPARATE archive database: a sema-engine keyed table over its own
/// `*.sema` file, distinct from the live intent log.
///
/// `CollectRemovalCandidates` opens one of these on demand at the
/// owner-configured [`ArchiveDatabaseTarget`], asserts each removal-candidate
/// `Entry` into it, and drops the handle when the collection completes. The
/// archive owns no relationship to the live `Store` database beyond holding the
/// records the live log let go.
struct ArchiveDatabase {
    database: SemaDatabase,
    entries: TableReference<StoredRecord>,
}

impl ArchiveDatabase {
    fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let mut database = SemaDatabase::open(EngineOpen::new(path.into(), SPIRIT_SCHEMA_VERSION))?;
        let entries = database.register_table(TableDescriptor::new(ENTRIES_TABLE))?;
        Ok(Self { database, entries })
    }

    /// Durably assert an archived copy of one removal-candidate `Entry` into the
    /// separate archive database. The archive allocates its own identifier; the
    /// original live identifier travels in the `CollectRemovalCandidates` reply,
    /// not in the archive's identifier space.
    fn archive_record(&mut self, record: StoredRecord) -> Result<(), StoreError> {
        self.database.assert(Assertion::new(self.entries, record))?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sema database engine error: {0}")]
    Database(#[from] sema_engine::Error),

    #[error("failed to encode record rkyv archive")]
    ArchiveEncode,

    #[error("failed to mint record identifier: {0}")]
    IdentifierMint(String),
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
