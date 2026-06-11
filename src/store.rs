#[cfg(feature = "agent-guardian")]
use std::collections::BTreeMap;
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

#[cfg(feature = "agent-guardian")]
use crate::guardian_journal::{GuardianDecision, GuardianJournal, GuardianOperation};
use crate::schema::{
    meta_signal::ArchiveDatabaseTarget,
    sema::{
        self as sema_schema, EngineStartFailure as SemaEngineStartFailure,
        EngineStopFailure as SemaEngineStopFailure, ReadInput as SemaReadInput,
        ReadOutput as SemaReadOutput, SemaEngine, WriteInput as SemaWriteInput,
        WriteOutput as SemaWriteOutput,
    },
    signal::{
        Certainty, CertaintyChange, CertaintyChangeReceipt, CertaintySelection, Clarification,
        ClarificationReceipt, CountedRecords, DatabaseMarker, Description, Entry, ErrorMessage,
        ErrorReport, Explanation, FoundRecord, GuardianRejection, GuardianRejectionReason,
        Importance, ImportanceBump, ImportanceBumpReceipt, ImportanceSelection, Keyword,
        KeywordMatch, Keywords, Magnitude, ObservedRecord, ObservedRecords, Privacy,
        PrivacySelection, Query, RecordChange, RecordChangeReceipt, RecordCount, RecordIdentifier,
        RecordSet, Referent, ReferentRegistration, ReferentRegistrationReceipt, ReferentSelection,
        Referents, Removal, RemovalArchiveRecord, RemovalArchiveRecords,
        RemovalCandidateCollection, RemovalCandidatesCollection, RemoveReceipt, RemovedIdentifier,
        RemovedIdentifiers, Retirement, RetirementReceipt, SearchText, SemaReceipt,
        SkippedRemovalCandidate, SkippedRemovalCandidates, Supersession, SupersessionReceipt,
        TextMatch,
    },
};

#[cfg(feature = "agent-guardian")]
use crate::schema::signal::{RegisteredReferent, RegisteredReferents};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, TraceEvent, TraceLog, schema::sema::SemaObjectName};

const SPIRIT_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(8);
const ENTRIES_TABLE: TableName = TableName::new("records");
const REFERENTS_TABLE: TableName = TableName::new("referents");
#[cfg(feature = "agent-guardian")]
const GUARDIAN_RECORD_LIMIT: usize = 64;
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
    referents: TableReference<StoredReferent>,
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

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct StoredReferent {
    referent: Referent,
    aliases: Referents,
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
            SemaWriteInput::Record(record) => match self.record(record.into_payload()) {
                Ok(identifier) => SemaWriteOutput::recorded(SemaReceipt {
                    record_identifier: RecordIdentifier::new(identifier),
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaWriteOutput::missed(ErrorReport {
                    error_message: ErrorMessage::new(error.to_string()),
                    database_marker: self.database_marker(),
                }),
            },
            SemaWriteInput::Remove(remove) => {
                let record_identifier = remove.into_payload().record_identifier;
                match self.remove(record_identifier.payload()) {
                    Ok(true) => SemaWriteOutput::removed(RemoveReceipt {
                        record_identifier,
                        database_marker: self.database_marker(),
                    }),
                    Ok(false) => SemaWriteOutput::missed(ErrorReport {
                        error_message: ErrorMessage::new("record not found"),
                        database_marker: self.database_marker(),
                    }),
                    Err(error) => SemaWriteOutput::missed(ErrorReport {
                        error_message: ErrorMessage::new(error.to_string()),
                        database_marker: self.database_marker(),
                    }),
                }
            }
            SemaWriteInput::ChangeCertainty(change) => {
                match self.change_certainty(change.into_payload()) {
                    Ok(Some(receipt)) => SemaWriteOutput::certainty_changed(receipt),
                    Ok(None) => SemaWriteOutput::missed(ErrorReport {
                        error_message: ErrorMessage::new("record not found"),
                        database_marker: self.database_marker(),
                    }),
                    Err(error) => SemaWriteOutput::missed(ErrorReport {
                        error_message: ErrorMessage::new(error.to_string()),
                        database_marker: self.database_marker(),
                    }),
                }
            }
            SemaWriteInput::BumpImportance(change) => {
                match self.bump_importance(change.into_payload()) {
                    Ok(Some(receipt)) => SemaWriteOutput::importance_bumped(receipt),
                    Ok(None) => SemaWriteOutput::missed(ErrorReport {
                        error_message: ErrorMessage::new("record not found"),
                        database_marker: self.database_marker(),
                    }),
                    Err(error) => SemaWriteOutput::missed(ErrorReport {
                        error_message: ErrorMessage::new(error.to_string()),
                        database_marker: self.database_marker(),
                    }),
                }
            }
            SemaWriteInput::ChangeRecord(change) => match self.change_record(change.into_payload())
            {
                Ok(Some(receipt)) => SemaWriteOutput::record_changed(receipt),
                Ok(None) => SemaWriteOutput::missed(ErrorReport {
                    error_message: ErrorMessage::new("record not found"),
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaWriteOutput::missed(ErrorReport {
                    error_message: ErrorMessage::new(error.to_string()),
                    database_marker: self.database_marker(),
                }),
            },
            SemaWriteInput::RegisterReferent(register) => {
                match self.register_referent(register.into_payload()) {
                    Ok(receipt) => SemaWriteOutput::referent_registered(receipt),
                    Err(error) => SemaWriteOutput::missed(ErrorReport {
                        error_message: ErrorMessage::new(error.to_string()),
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
            SemaReadInput::Observe(observe) => match self.observe(observe.payload()) {
                Ok(entries) if !entries.is_empty() => SemaReadOutput::observed(ObservedRecords {
                    record_set: RecordSet::new(entries),
                    database_marker: self.database_marker(),
                }),
                Ok(_) => SemaReadOutput::missed(ErrorReport {
                    error_message: ErrorMessage::new("no matching record"),
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaReadOutput::missed(ErrorReport {
                    error_message: ErrorMessage::new(error.to_string()),
                    database_marker: self.database_marker(),
                }),
            },
            SemaReadInput::Lookup(lookup) => {
                let record_identifier = lookup.into_payload();
                match self.entry_by_identifier(record_identifier.payload()) {
                    Ok(Some(entry)) => SemaReadOutput::found(FoundRecord {
                        record_identifier,
                        entry,
                        database_marker: self.database_marker(),
                    }),
                    Ok(None) => SemaReadOutput::missed(ErrorReport {
                        error_message: ErrorMessage::new("record not found"),
                        database_marker: self.database_marker(),
                    }),
                    Err(error) => SemaReadOutput::missed(ErrorReport {
                        error_message: ErrorMessage::new(error.to_string()),
                        database_marker: self.database_marker(),
                    }),
                }
            }
            SemaReadInput::Count(count) => match self.count(count.payload()) {
                Ok(count) => SemaReadOutput::counted(CountedRecords {
                    record_count: RecordCount::new(count),
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaReadOutput::missed(ErrorReport {
                    error_message: ErrorMessage::new(error.to_string()),
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
        let referents = database.register_table(TableDescriptor::new(REFERENTS_TABLE))?;
        Ok(Self {
            database,
            entries,
            referents,
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
            ArchiveDatabaseTarget::Path(archive_path) => {
                PathBuf::from(archive_path.payload().payload())
            }
        }
    }

    #[cfg(feature = "agent-guardian")]
    fn guardian_journal_path(&self) -> PathBuf {
        let stem = self
            .path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("spirit"));
        self.path.with_file_name(format!("{stem}.guardian.sema"))
    }

    #[cfg(feature = "agent-guardian")]
    fn open_guardian_journal(&self) -> Result<GuardianJournal, StoreError> {
        GuardianJournal::open(self.guardian_journal_path())
    }

    #[cfg(feature = "agent-guardian")]
    pub(crate) fn record_guardian_decision(
        &self,
        decision: GuardianDecision,
    ) -> Result<(), StoreError> {
        self.open_guardian_journal()?.append(decision)
    }

    #[cfg(feature = "agent-guardian")]
    pub fn guardian_decision_count(&self) -> Result<usize, StoreError> {
        self.open_guardian_journal()?.len()
    }

    #[cfg(feature = "agent-guardian")]
    pub(crate) fn registered_referents(&self) -> Result<RegisteredReferents, StoreError> {
        Ok(RegisteredReferents::new(
            self.referents()?
                .into_iter()
                .map(StoredReferent::into_registered_referent)
                .collect(),
        ))
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
    /// [`SkippedRemovalCandidate`] with
    /// `RemovalCandidateSkipReason::ArchiveFailed`; a record that vanishes
    /// between the match and the retraction is reported with
    /// `RemovalCandidateSkipReason::RecordAlreadyRemoved`. The reply
    /// carries the archived records, the removed identifiers, the skipped
    /// candidates, and the live database's post-removal marker.
    pub fn collect_removal_candidates(
        &self,
        collection: RemovalCandidateCollection,
    ) -> Result<RemovalCandidatesCollection, StoreError> {
        let query = collection.record_query.into_payload();
        let mut archive = self.open_archive_database()?;
        let mut archived_records = Vec::new();
        let mut removed_identifiers = Vec::new();
        let mut skipped_candidates = Vec::new();
        for record in self.records()? {
            let identifier = record.record_identifier.clone();
            if !record.entry.matches(&query) {
                continue;
            }
            match archive.archive_record(record.clone(), self.archive_identifier(&identifier)) {
                Ok(()) => match self.remove(&identifier)? {
                    true => {
                        archived_records.push(RemovalArchiveRecord {
                            record_identifier: RecordIdentifier::new(identifier.clone()),
                            entry: record.entry,
                        });
                        removed_identifiers
                            .push(RemovedIdentifier::new(RecordIdentifier::new(identifier)));
                    }
                    false => skipped_candidates.push(SkippedRemovalCandidate {
                        record_identifier: RecordIdentifier::new(identifier.clone()),
                        removal_candidate_skip_reason:
                            crate::schema::signal::RemovalCandidateSkipReason::RecordAlreadyRemoved,
                    }),
                },
                Err(_error) => skipped_candidates.push(SkippedRemovalCandidate {
                    record_identifier: RecordIdentifier::new(identifier.clone()),
                    removal_candidate_skip_reason:
                        crate::schema::signal::RemovalCandidateSkipReason::ArchiveFailed,
                }),
            }
        }
        Ok(RemovalCandidatesCollection {
            removal_archive_records: RemovalArchiveRecords::new(archived_records),
            removed_identifiers: RemovedIdentifiers::new(removed_identifiers),
            skipped_removal_candidates: SkippedRemovalCandidates::new(skipped_candidates),
            database_marker: self.database_marker(),
        })
    }

    /// Open the SEPARATE archive database at the owner-configured target. This
    /// is a distinct `sema-engine` handle over a distinct `*.sema` file; it is
    /// never the live database handle.
    fn open_archive_database(&self) -> Result<ArchiveDatabase, StoreError> {
        ArchiveDatabase::open(self.archive_database_path())
    }

    fn archive_identifier(&self, live_identifier: &str) -> String {
        format!(
            "{live_identifier}-{}",
            self.database_marker().commit_sequence.payload()
        )
    }

    pub fn import_record(
        &self,
        record_identifier: String,
        entry: Entry,
    ) -> Result<String, StoreError> {
        let entry = self.canonicalized_entry(entry)?;
        self.database.assert(Assertion::new(
            self.entries,
            StoredRecord::new(record_identifier.clone(), entry),
        ))?;
        Ok(record_identifier)
    }

    pub fn register_referent(
        &self,
        registration: ReferentRegistration,
    ) -> Result<ReferentRegistrationReceipt, StoreError> {
        let mut record = StoredReferent::new(registration.referent, registration.aliases);
        self.reject_conflicting_referent_names(&record)?;
        if let Some(existing) = self.referent_by_key(record.referent.payload())? {
            record = existing.with_aliases_merged(record.aliases);
            self.database
                .mutate(Mutation::new(self.referents, record.clone()))?;
        } else {
            self.database
                .assert(Assertion::new(self.referents, record.clone()))?;
        }
        Ok(ReferentRegistrationReceipt {
            referent: record.referent,
            database_marker: self.database_marker(),
        })
    }

    fn record(&self, entry: Entry) -> Result<String, StoreError> {
        let record_identifier = self.next_record_identifier()?;
        self.import_record(record_identifier.clone(), entry)?;
        Ok(record_identifier)
    }

    pub fn propose(&self, entry: Entry) -> Result<SemaReceipt, StoreError> {
        self.record_entry(entry)
    }

    pub fn record_entry(&self, entry: Entry) -> Result<SemaReceipt, StoreError> {
        let record_identifier = RecordIdentifier::new(self.record(entry)?);
        Ok(SemaReceipt {
            record_identifier,
            database_marker: self.database_marker(),
        })
    }

    pub fn guard_propose(
        &self,
        entry: Entry,
    ) -> Result<Result<SemaReceipt, GuardianRejection>, StoreError> {
        let Some(duplicate) = self.duplicate_record(&entry)? else {
            return Ok(Ok(self.propose(entry)?));
        };
        let record_identifier = RecordIdentifier::new(duplicate.record_identifier.clone());
        let importance_receipt = self
            .bump_importance(ImportanceBump::new(record_identifier.clone()))?
            .ok_or_else(|| {
                StoreError::DuplicateRecordVanished(record_identifier.payload().clone())
            })?;
        let updated_entry = self
            .entry_by_identifier(record_identifier.payload())?
            .ok_or_else(|| {
                StoreError::DuplicateRecordVanished(record_identifier.payload().clone())
            })?;
        Ok(Err(GuardianRejection {
            guardian_rejection_reason: GuardianRejectionReason::Duplicate,
            record_set: RecordSet::new(vec![ObservedRecord {
                record_identifier,
                entry: updated_entry,
            }]),
            explanation: Explanation::new("proposal duplicates an existing forward arrow"),
            database_marker: importance_receipt.database_marker,
        }))
    }

    fn observe(&self, query: &Query) -> Result<Vec<ObservedRecord>, StoreError> {
        let query = self.canonicalized_query(query.clone())?;
        let mut records = self
            .records()?
            .into_iter()
            .filter(|record| record.entry.matches(&query))
            .collect::<Vec<_>>();
        records.sort_by_key(|record| {
            std::cmp::Reverse((
                record.entry.certainty_rank(),
                record.entry.importance_rank(),
            ))
        });
        Ok(records
            .into_iter()
            .map(StoredRecord::into_observed_record)
            .collect())
    }

    #[cfg(feature = "agent-guardian")]
    pub(crate) fn guardian_records_for_operation(
        &self,
        operation: &GuardianOperation,
    ) -> Result<RecordSet, StoreError> {
        let mut bundle = GuardianRecordBundle::new();
        if let Some(candidate) = operation.candidate_entry() {
            bundle.extend(self.guardian_records_for_entry(candidate)?);
        }
        match operation {
            GuardianOperation::Clarify(clarification) => {
                if let Some(current) =
                    self.observed_record_by_identifier(clarification.record_identifier.payload())?
                {
                    bundle.insert(current.clone());
                    let mut candidate = current.entry;
                    candidate.description = clarification.description.clone();
                    bundle.extend(self.guardian_records_for_entry(&candidate)?);
                }
            }
            GuardianOperation::Supersede(supersession) => {
                for retired_identifier in supersession.retired_identifiers.payload() {
                    if let Some(current) =
                        self.observed_record_by_identifier(retired_identifier.payload().payload())?
                    {
                        bundle.insert(current);
                    }
                }
            }
            GuardianOperation::Retire(retirement) => {
                if let Some(current) =
                    self.observed_record_by_identifier(retirement.record_identifier.payload())?
                {
                    bundle.insert(current);
                }
            }
            GuardianOperation::Remove(removal) => {
                if let Some(current) =
                    self.observed_record_by_identifier(removal.record_identifier.payload())?
                {
                    bundle.insert(current);
                }
            }
            GuardianOperation::ChangeRecord(change) => {
                if let Some(current) =
                    self.observed_record_by_identifier(change.record_identifier.payload())?
                {
                    bundle.insert(current);
                }
            }
            GuardianOperation::CollectRemovalCandidates(collection) => {
                let mut records = self.observe(collection.record_query.payload())?;
                records.truncate(GUARDIAN_RECORD_LIMIT);
                bundle.extend(RecordSet::new(records));
            }
            GuardianOperation::Record(_) | GuardianOperation::Propose(_) => {}
        }
        Ok(bundle.into_record_set())
    }

    #[cfg(feature = "agent-guardian")]
    fn guardian_records_for_entry(&self, proposed: &Entry) -> Result<RecordSet, StoreError> {
        let proposed = self.canonicalized_entry(proposed.clone())?;
        let mut records = self
            .records()?
            .into_iter()
            .filter_map(|record| GuardianRecordCandidate::new(record, &proposed))
            .collect::<Vec<_>>();
        records.sort_by_key(GuardianRecordCandidate::sort_key);
        Ok(RecordSet::new(
            records
                .into_iter()
                .take(GUARDIAN_RECORD_LIMIT)
                .map(GuardianRecordCandidate::into_observed_record)
                .collect(),
        ))
    }

    #[cfg(feature = "agent-guardian")]
    fn observed_record_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<ObservedRecord>, StoreError> {
        Ok(self
            .database
            .match_records(QueryPlan::key(self.entries, RecordKey::new(identifier)))?
            .records()
            .iter()
            .next()
            .cloned()
            .map(StoredRecord::into_observed_record))
    }

    fn duplicate_record(&self, proposed: &Entry) -> Result<Option<StoredRecord>, StoreError> {
        let proposed = self.canonicalized_entry(proposed.clone())?;
        Ok(self.records()?.into_iter().find(|record| {
            record.entry.kind == proposed.kind
                && record.entry.domains == proposed.domains
                && record
                    .entry
                    .description
                    .payload()
                    .trim()
                    .eq_ignore_ascii_case(proposed.description.payload().trim())
        }))
    }

    #[cfg(feature = "agent-guardian")]
    pub(crate) fn apply_duplicate_guardian_rejection(
        &self,
        entry: &Entry,
        fallback: GuardianRejection,
    ) -> Result<GuardianRejection, StoreError> {
        let Some(duplicate) = self.duplicate_record(entry)? else {
            return Ok(fallback);
        };
        let record_identifier = RecordIdentifier::new(duplicate.record_identifier.clone());
        let importance_receipt = self
            .bump_importance(ImportanceBump::new(record_identifier.clone()))?
            .ok_or_else(|| {
                StoreError::DuplicateRecordVanished(record_identifier.payload().clone())
            })?;
        let updated_entry = self
            .entry_by_identifier(record_identifier.payload())?
            .ok_or_else(|| {
                StoreError::DuplicateRecordVanished(record_identifier.payload().clone())
            })?;
        Ok(GuardianRejection {
            guardian_rejection_reason: GuardianRejectionReason::Duplicate,
            record_set: RecordSet::new(vec![ObservedRecord {
                record_identifier,
                entry: updated_entry,
            }]),
            explanation: Explanation::new("guardian judged the write as a duplicate"),
            database_marker: importance_receipt.database_marker,
        })
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

    pub(crate) fn remove_record(
        &self,
        removal: Removal,
    ) -> Result<Option<RemoveReceipt>, StoreError> {
        let record_identifier = removal.record_identifier;
        if !self.remove(record_identifier.payload())? {
            return Ok(None);
        }
        Ok(Some(RemoveReceipt {
            record_identifier,
            database_marker: self.database_marker(),
        }))
    }

    pub fn retire(&self, retirement: Retirement) -> Result<Option<RetirementReceipt>, StoreError> {
        let record_identifier = retirement.record_identifier;
        let Some(entry) = self.entry_by_identifier(record_identifier.payload())? else {
            return Ok(None);
        };
        let mut archive = self.open_archive_database()?;
        archive.archive_record(
            StoredRecord::new(record_identifier.payload().clone(), entry),
            self.archive_identifier(record_identifier.payload()),
        )?;
        if !self.remove(record_identifier.payload())? {
            return Ok(None);
        }
        Ok(Some(RetirementReceipt {
            record_identifier,
            database_marker: self.database_marker(),
        }))
    }

    fn change_certainty(
        &self,
        change: CertaintyChange,
    ) -> Result<Option<CertaintyChangeReceipt>, StoreError> {
        let record_identifier = change.record_identifier;
        let identifier_text = record_identifier.payload().clone();
        let Some(mut entry) = self.entry_by_identifier(record_identifier.payload())? else {
            return Ok(None);
        };
        entry.certainty = change.certainty.clone();
        self.database.mutate(Mutation::new(
            self.entries,
            StoredRecord::new(identifier_text, entry),
        ))?;
        Ok(Some(CertaintyChangeReceipt {
            record_identifier,
            certainty: change.certainty,
            database_marker: self.database_marker(),
        }))
    }

    fn bump_importance(
        &self,
        change: ImportanceBump,
    ) -> Result<Option<ImportanceBumpReceipt>, StoreError> {
        let record_identifier = change.into_payload();
        let identifier_text = record_identifier.payload().clone();
        let Some(mut entry) = self.entry_by_identifier(record_identifier.payload())? else {
            return Ok(None);
        };
        entry.importance = entry.importance.next();
        let importance = entry.importance.clone();
        self.database.mutate(Mutation::new(
            self.entries,
            StoredRecord::new(identifier_text, entry),
        ))?;
        Ok(Some(ImportanceBumpReceipt {
            record_identifier,
            importance,
            database_marker: self.database_marker(),
        }))
    }

    pub(crate) fn change_record(
        &self,
        change: RecordChange,
    ) -> Result<Option<RecordChangeReceipt>, StoreError> {
        let record_identifier = change.record_identifier;
        let identifier_text = record_identifier.payload().clone();
        if self
            .entry_by_identifier(record_identifier.payload())?
            .is_none()
        {
            return Ok(None);
        }
        let entry = self.canonicalized_entry(change.entry)?;
        self.database.mutate(Mutation::new(
            self.entries,
            StoredRecord::new(identifier_text, entry),
        ))?;
        Ok(Some(RecordChangeReceipt {
            record_identifier,
            database_marker: self.database_marker(),
        }))
    }

    pub fn clarify(
        &self,
        clarification: Clarification,
    ) -> Result<Option<ClarificationReceipt>, StoreError> {
        let record_identifier = clarification.record_identifier;
        let identifier_text = record_identifier.payload().clone();
        let Some(mut entry) = self.entry_by_identifier(record_identifier.payload())? else {
            return Ok(None);
        };
        let mut archive = self.open_archive_database()?;
        archive.archive_record(
            StoredRecord::new(identifier_text.clone(), entry.clone()),
            self.archive_identifier(record_identifier.payload()),
        )?;
        entry.description = clarification.description;
        self.database.mutate(Mutation::new(
            self.entries,
            StoredRecord::new(identifier_text, entry),
        ))?;
        Ok(Some(ClarificationReceipt {
            record_identifier,
            database_marker: self.database_marker(),
        }))
    }

    pub fn supersede(
        &self,
        supersession: Supersession,
    ) -> Result<Option<SupersessionReceipt>, StoreError> {
        let retired_identifiers = supersession.retired_identifiers;
        let replacement = supersession.replacement;
        let mut archive = self.open_archive_database()?;
        for identifier in retired_identifiers.payload() {
            let Some(entry) = self.entry_by_identifier(identifier.payload().payload())? else {
                return Ok(None);
            };
            archive.archive_record(
                StoredRecord::new(identifier.payload().payload().clone(), entry),
                self.archive_identifier(identifier.payload().payload()),
            )?;
        }
        for identifier in retired_identifiers.payload() {
            self.remove(identifier.payload().payload())?;
        }
        let sema_receipt = self.propose(replacement)?;
        Ok(Some(SupersessionReceipt {
            retired_identifiers,
            sema_receipt,
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
            commit_sequence: crate::schema::signal::CommitSequence::new(
                self.commit_sequence().unwrap_or(0),
            ),
            state_digest: crate::schema::signal::StateDigest::new(self.state_digest().unwrap_or(0)),
        }
    }

    fn commit_sequence(&self) -> Result<u64, StoreError> {
        Ok(self.database.current_commit_sequence()?.value())
    }

    /// A content-addressed digest of committed state: blake3 over each
    /// record's `(identifier, archived bytes)` and registered referent,
    /// folded with the commit
    /// sequence, reduced to the schema's `Integer` digest width. An empty
    /// store (no committed records) digests to zero, so a marker taken
    /// before any write reads `(0, 0)`.
    fn state_digest(&self) -> Result<u64, StoreError> {
        let records = self.records()?;
        let referents = self.referents()?;
        if records.is_empty() && referents.is_empty() {
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
        for referent in referents {
            let archive = rkyv::to_bytes::<rkyv::rancor::Error>(&referent)
                .map_err(|_| StoreError::ArchiveEncode)?;
            hasher.update(referent.referent.payload().as_bytes());
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

    fn referents(&self) -> Result<Vec<StoredReferent>, StoreError> {
        Ok(self
            .database
            .match_records(QueryPlan::all(self.referents))?
            .records()
            .to_vec())
    }

    fn referent_by_key(&self, referent: &str) -> Result<Option<StoredReferent>, StoreError> {
        Ok(self
            .database
            .match_records(QueryPlan::key(self.referents, RecordKey::new(referent)))?
            .records()
            .iter()
            .next()
            .cloned())
    }

    fn reject_conflicting_referent_names(
        &self,
        candidate: &StoredReferent,
    ) -> Result<(), StoreError> {
        for existing in self.referents()? {
            if existing.referent == candidate.referent {
                continue;
            }
            if existing.has_any_name(candidate) {
                return Err(StoreError::ReferentNameConflict(
                    candidate.referent.payload().clone(),
                ));
            }
        }
        Ok(())
    }

    fn canonicalized_entry(&self, mut entry: Entry) -> Result<Entry, StoreError> {
        entry.referents = self.canonicalized_referents(entry.referents)?;
        Ok(entry)
    }

    fn canonicalized_query(&self, mut query: Query) -> Result<Query, StoreError> {
        query.referent_selection =
            self.canonicalized_referent_selection(query.referent_selection)?;
        Ok(query)
    }

    fn canonicalized_referent_selection(
        &self,
        selection: ReferentSelection,
    ) -> Result<ReferentSelection, StoreError> {
        match selection {
            ReferentSelection::Any => Ok(ReferentSelection::Any),
            ReferentSelection::AnyReferent(referents) => Ok(ReferentSelection::any_referent(
                self.canonicalized_referents(referents.into_payload())?,
            )),
            ReferentSelection::AllReferents(referents) => Ok(ReferentSelection::all_referents(
                self.canonicalized_referents(referents.into_payload())?,
            )),
        }
    }

    fn canonicalized_referents(&self, referents: Referents) -> Result<Referents, StoreError> {
        let mut canonical = Vec::new();
        for referent in referents.into_payload() {
            canonical.push(
                self.canonical_referent(&referent)?
                    .ok_or_else(|| StoreError::UnregisteredReferent(referent.payload().clone()))?,
            );
        }
        Ok(Referents::new(canonical))
    }

    fn canonical_referent(&self, referent: &Referent) -> Result<Option<Referent>, StoreError> {
        Ok(self
            .referents()?
            .into_iter()
            .find(|registered| registered.matches(referent))
            .map(|registered| registered.referent))
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

    fn into_observed_record(self) -> ObservedRecord {
        ObservedRecord {
            record_identifier: RecordIdentifier::new(self.record_identifier),
            entry: self.entry,
        }
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

impl StoredReferent {
    fn new(referent: Referent, aliases: Referents) -> Self {
        Self { referent, aliases }
    }

    fn with_aliases_merged(mut self, aliases: Referents) -> Self {
        let mut merged = self.aliases.into_payload();
        for alias in aliases.into_payload() {
            if alias != self.referent && !merged.contains(&alias) {
                merged.push(alias);
            }
        }
        self.aliases = Referents::new(merged);
        self
    }

    fn matches(&self, referent: &Referent) -> bool {
        &self.referent == referent || self.aliases.payload().contains(referent)
    }

    fn has_any_name(&self, other: &StoredReferent) -> bool {
        self.matches(&other.referent)
            || other
                .aliases
                .payload()
                .iter()
                .any(|alias| self.matches(alias))
    }

    #[cfg(feature = "agent-guardian")]
    fn into_registered_referent(self) -> RegisteredReferent {
        RegisteredReferent {
            referent: self.referent,
            aliases: self.aliases,
        }
    }
}

impl EngineRecord for StoredReferent {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.referent.payload().clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "agent-guardian")]
struct GuardianRecordBundle {
    records: BTreeMap<String, ObservedRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "agent-guardian")]
struct GuardianRecordCandidate {
    score: u64,
    record_identifier: String,
    observed_record: ObservedRecord,
}

#[cfg(feature = "agent-guardian")]
impl GuardianRecordBundle {
    fn new() -> Self {
        Self {
            records: BTreeMap::new(),
        }
    }

    fn insert(&mut self, record: ObservedRecord) {
        self.records
            .insert(record.record_identifier.payload().clone(), record);
    }

    fn extend(&mut self, record_set: RecordSet) {
        for record in record_set.into_payload() {
            self.insert(record);
        }
    }

    fn into_record_set(self) -> RecordSet {
        RecordSet::new(self.records.into_values().collect())
    }
}

#[cfg(feature = "agent-guardian")]
impl GuardianRecordCandidate {
    fn new(record: StoredRecord, proposed: &Entry) -> Option<Self> {
        let score = record.entry.guardian_relevance_score(proposed);
        if score == 0 {
            return None;
        }
        let record_identifier = record.record_identifier.clone();
        Some(Self {
            score,
            record_identifier,
            observed_record: record.into_observed_record(),
        })
    }

    fn sort_key(
        &self,
    ) -> (
        std::cmp::Reverse<u64>,
        std::cmp::Reverse<u64>,
        std::cmp::Reverse<u64>,
        String,
    ) {
        (
            std::cmp::Reverse(self.score),
            std::cmp::Reverse(self.observed_record.entry.certainty_rank()),
            std::cmp::Reverse(self.observed_record.entry.importance_rank()),
            self.record_identifier.clone(),
        )
    }

    fn into_observed_record(self) -> ObservedRecord {
        self.observed_record
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

    /// Durably assert an archived copy of one live `Entry` into the separate
    /// archive database under a versioned archive key, so repeated clarification
    /// and retirement of the same live identifier preserve every prior state.
    fn archive_record(
        &mut self,
        record: StoredRecord,
        archive_identifier: String,
    ) -> Result<(), StoreError> {
        self.database.assert(Assertion::new(
            self.entries,
            StoredRecord::new(archive_identifier, record.entry),
        ))?;
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

    #[error("unregistered referent: {0}")]
    UnregisteredReferent(String),

    #[error("referent name already registered under another canonical referent: {0}")]
    ReferentNameConflict(String),

    #[error("duplicate record vanished during guardian proposal handling: {0}")]
    DuplicateRecordVanished(String),
}

impl Entry {
    pub fn matches(&self, query: &Query) -> bool {
        query.matches(self)
    }

    pub fn certainty_rank(&self) -> u64 {
        self.certainty.payload().rank()
    }

    pub fn importance_rank(&self) -> u64 {
        self.importance.payload().rank()
    }

    #[cfg(feature = "agent-guardian")]
    fn guardian_relevance_score(&self, proposed: &Entry) -> u64 {
        if !CertaintySelection::default_observation_certainty().matches(&self.certainty) {
            return 0;
        }
        let mut score = 0;
        if self.duplicates(proposed) {
            score += 100;
        }
        if self.shares_domain(proposed) {
            score += 30;
        }
        if self.shares_referent(proposed) {
            score += 30;
        }
        if self.shares_keyword(proposed) {
            score += 20;
        }
        if self.shares_text(proposed) {
            score += 10;
        }
        if self.kind == proposed.kind {
            score += 1;
        }
        score
    }

    #[cfg(feature = "agent-guardian")]
    fn duplicates(&self, proposed: &Entry) -> bool {
        self.kind == proposed.kind
            && self.domains == proposed.domains
            && self
                .description
                .payload()
                .trim()
                .eq_ignore_ascii_case(proposed.description.payload().trim())
    }

    #[cfg(feature = "agent-guardian")]
    fn shares_domain(&self, proposed: &Entry) -> bool {
        self.domains
            .payload()
            .iter()
            .any(|domain| proposed.domains.payload().contains(domain))
    }

    #[cfg(feature = "agent-guardian")]
    fn shares_referent(&self, proposed: &Entry) -> bool {
        !self.referents.payload().is_empty()
            && self
                .referents
                .payload()
                .iter()
                .any(|referent| proposed.referents.payload().contains(referent))
    }

    #[cfg(feature = "agent-guardian")]
    fn shares_keyword(&self, proposed: &Entry) -> bool {
        self.description
            .keywords()
            .contains_any(&proposed.description.keywords())
    }

    #[cfg(feature = "agent-guardian")]
    fn shares_text(&self, proposed: &Entry) -> bool {
        self.description
            .contains_description_text(&proposed.description)
            || proposed
                .description
                .contains_description_text(&self.description)
    }
}

impl Description {
    pub fn keywords(&self) -> Keywords {
        let mut keywords = Vec::new();
        let mut seen = BTreeSet::new();
        let mut inside_keyword = false;
        let mut keyword = String::new();
        for character in self.payload().chars() {
            if character == '*' {
                if inside_keyword {
                    let normalized = keyword.trim().to_lowercase();
                    if !normalized.is_empty() && seen.insert(normalized.clone()) {
                        keywords.push(Keyword::new(normalized));
                    }
                    keyword.clear();
                    inside_keyword = false;
                } else {
                    keyword.clear();
                    inside_keyword = true;
                }
            } else if inside_keyword {
                keyword.push(character);
            }
        }
        Keywords::new(keywords)
    }

    pub fn contains_search_text(&self, search_text: &SearchText) -> bool {
        self.payload()
            .to_lowercase()
            .contains(&search_text.payload().trim().to_lowercase())
    }

    #[cfg(feature = "agent-guardian")]
    pub fn contains_description_text(&self, other: &Description) -> bool {
        let other = other.payload().trim();
        !other.is_empty() && self.contains_search_text(&SearchText::new(other))
    }
}

impl Keyword {
    pub fn normalized(&self) -> String {
        self.payload().trim().to_lowercase()
    }
}

impl Keywords {
    pub fn contains_keyword(&self, expected: &Keyword) -> bool {
        let expected = expected.normalized();
        self.payload()
            .iter()
            .any(|keyword| keyword.normalized() == expected)
    }

    pub fn contains_any(&self, expected: &Keywords) -> bool {
        expected
            .payload()
            .iter()
            .any(|keyword| self.contains_keyword(keyword))
    }

    pub fn contains_all(&self, expected: &Keywords) -> bool {
        expected
            .payload()
            .iter()
            .all(|keyword| self.contains_keyword(keyword))
    }
}

impl Query {
    pub fn matches(&self, entry: &Entry) -> bool {
        self.domain_match.matches(&entry.domains)
            && self.keyword_match.matches(&entry.description)
            && self.text_match.matches(&entry.description)
            && self.referent_selection.matches(&entry.referents)
            && self.kind.as_ref().is_none_or(|kind| &entry.kind == kind)
            && self.privacy_selection.matches(&entry.privacy)
            && self.certainty_selection.matches(&entry.certainty)
            && self.importance_selection.matches(&entry.importance)
    }
}

impl ReferentSelection {
    pub fn matches(&self, entry_referents: &Referents) -> bool {
        match self {
            Self::Any => true,
            Self::AnyReferent(expected) => expected
                .payload()
                .payload()
                .iter()
                .any(|referent| entry_referents.payload().contains(referent)),
            Self::AllReferents(expected) => expected
                .payload()
                .payload()
                .iter()
                .all(|referent| entry_referents.payload().contains(referent)),
        }
    }
}

impl KeywordMatch {
    pub fn matches(&self, description: &Description) -> bool {
        match self {
            Self::Any => true,
            Self::AnyKeyword(expected) => description.keywords().contains_any(expected.payload()),
            Self::AllKeywords(expected) => description.keywords().contains_all(expected.payload()),
        }
    }
}

impl TextMatch {
    pub fn matches(&self, description: &Description) -> bool {
        match self {
            Self::Any => true,
            Self::ContainsText(search_text) => {
                description.contains_search_text(search_text.payload())
            }
        }
    }
}

impl PrivacySelection {
    pub fn default_observation_privacy() -> Self {
        Self::exact(Privacy::new(Magnitude::Zero))
    }

    pub fn matches(&self, privacy: &Privacy) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(expected) => privacy == expected.payload(),
            Self::AtMost(maximum) => privacy.payload().rank() <= maximum.payload().payload().rank(),
            Self::AtLeast(minimum) => {
                privacy.payload().rank() >= minimum.payload().payload().rank()
            }
        }
    }
}

impl CertaintySelection {
    pub fn default_observation_certainty() -> Self {
        Self::at_least_certainty(Certainty::new(Magnitude::Minimum))
    }

    pub fn removal_candidate_certainty() -> Self {
        Self::exact_certainty(Certainty::new(Magnitude::Zero))
    }

    pub fn matches(&self, certainty: &Certainty) -> bool {
        let certainty = certainty.payload();
        match self {
            Self::Any => true,
            Self::ExactCertainty(expected) => certainty == expected.payload().payload(),
            Self::AtMostCertainty(maximum) => {
                certainty.rank() <= maximum.payload().payload().rank()
            }
            Self::AtLeastCertainty(minimum) => {
                certainty.rank() >= minimum.payload().payload().rank()
            }
        }
    }
}

impl ImportanceSelection {
    pub fn default_observation_importance() -> Self {
        Self::Any
    }

    pub fn matches(&self, importance: &Importance) -> bool {
        let importance = importance.payload();
        match self {
            Self::Any => true,
            Self::ExactImportance(expected) => importance == expected.payload().payload(),
            Self::AtMostImportance(maximum) => {
                importance.rank() <= maximum.payload().payload().rank()
            }
            Self::AtLeastImportance(minimum) => {
                importance.rank() >= minimum.payload().payload().rank()
            }
        }
    }
}

impl Importance {
    pub fn next(&self) -> Self {
        Self::new(self.payload().next())
    }
}

impl Magnitude {
    pub fn rank(&self) -> u64 {
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

    pub fn next(&self) -> Self {
        match self {
            Self::Zero => Self::Minimum,
            Self::Minimum => Self::VeryLow,
            Self::VeryLow => Self::Low,
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::VeryHigh,
            Self::VeryHigh | Self::Maximum => Self::Maximum,
        }
    }
}
