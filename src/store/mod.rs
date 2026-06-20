mod archive;
mod error;
mod family_directory;
#[cfg(feature = "agent-guardian")]
mod guardian_bundle;
mod record_identifier;

use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use sema_engine::{
    Assertion, Checkpoint, CheckpointReceipt, CommitSequence, Engine as SemaDatabase, EngineOpen,
    EngineRecord, Mutation, QueryPlan, RecordKey, Retraction, SchemaVersion, TableReference,
    VersionedCommitLogEntry,
};
#[cfg(feature = "mirror-shipper")]
use sema_engine::{EntryDigest, PortableCheckpoint};

pub(crate) use archive::ArchiveDatabase;
pub use error::StoreError;
pub use family_directory::StoreFamilyDirectory;
#[cfg(feature = "agent-guardian")]
use guardian_bundle::GuardianRecordBundle;
use record_identifier::RecordIdentifierMint;

#[cfg(feature = "agent-guardian")]
use crate::guardian_journal::{GuardianDecision, GuardianJournal, GuardianOperation};
use crate::schema::{
    meta_signal::ArchiveDatabaseTarget,
    sema::{
        self as sema_schema, EngineStartFailure as SemaEngineStartFailure,
        EngineStopFailure as SemaEngineStopFailure, Migration, ReadInput as SemaReadInput,
        ReadOutput as SemaReadOutput, RecordFamily, SemaEngine, StoredRecord, StoredReferent,
        WriteInput as SemaWriteInput, WriteOutput as SemaWriteOutput,
    },
    signal::{
        Certainty, CertaintyChange, CertaintyChangeReceipt, CertaintySelection, Clarification,
        ClarificationReceipt, ClarificationRecordIdentifier, ClarificationResolution,
        ClarificationResolutionReceipt, CountedRecords, DatabaseMarker, Description, Entry,
        ErrorMessage, ErrorReport, Explanation, FoundRecord, GuardianRejection,
        GuardianRejectionReason, Importance, ImportanceBump, ImportanceBumpReceipt,
        ImportanceSelection, Keyword, KeywordMatch, Keywords, Magnitude, ObservedRecord,
        ObservedRecords, Privacy, PrivacySelection, Query, RecordChange, RecordChangeReceipt,
        RecordCount, RecordIdentifier, RecordIdentifiers, RecordSet, Referent,
        ReferentRegistration, ReferentRegistrationReceipt, ReferentSelection, Referents, Removal,
        RemovalArchiveRecord, RemovalArchiveRecords, RemovalCandidateCollection,
        RemovalCandidatesCollection, RemoveReceipt, RemovedIdentifier, RemovedIdentifiers,
        Retirement, RetirementReceipt, SearchText, SemaReceipt, SkippedRemovalCandidate,
        SkippedRemovalCandidates, Supersession, SupersessionReceipt, TextMatch,
    },
};

const PUBLIC_TEXT_SEARCH_LIMIT: usize = 25;

#[cfg(feature = "agent-guardian")]
use crate::schema::signal::{
    DomainMatch, DomainScope, DomainScopes, RegisteredReferent, RegisteredReferents, SelectedKind,
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, TraceEvent, TraceLog, schema::sema::SemaObjectName};

// Version 9 is the versioned-store bootstrap: the store opts into the
// sema-engine versioned commit log through the schema-generated family
// descriptors and versioning policy, so every durable write from this version
// on is replayable history. Version 8 and earlier are pre-versioning stores
// readable only through `sema-engine-previous` in `production_migration`.
//
// Version 10 coarsens the stored Technology/Software domain enum: fine leaves
// that belonged in keywords are folded into terminal-able sub-domain values.
pub(super) const SPIRIT_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(10);

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
    // The engine serializes its own writes; after startup table registration the
    // store no longer needs exclusive ownership of the database handle.
    database: Arc<SemaDatabase>,
    entries: TableReference<StoredRecord>,
    referents: TableReference<StoredReferent>,
    migrations: TableReference<Migration>,
    path: PathBuf,
    archive_target: ArchiveDatabaseTarget,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
}

#[cfg(feature = "mirror-shipper")]
struct MirrorRestoreImport {
    checkpoint: Checkpoint,
    suffix: Vec<VersionedCommitLogEntry>,
    restored_head: EntryDigest,
}

#[cfg(feature = "mirror-shipper")]
impl MirrorRestoreImport {
    fn from_bundle(bundle: signal_mirror::RestoreBundle) -> Result<Self, StoreError> {
        let checkpoint =
            PortableCheckpoint::from_bytes(bundle.checkpoint.artifact.as_slice().to_vec())
                .decode()?;
        let restored_head = bundle
            .suffix()
            .last()
            .map(|envelope| EntryDigest::new(*envelope.digest.as_bytes()))
            .unwrap_or_else(|| checkpoint.metadata().covered_entry_digest());
        let suffix = bundle
            .into_suffix()
            .into_iter()
            .map(|envelope| {
                rkyv::from_bytes::<VersionedCommitLogEntry, rkyv::rancor::Error>(
                    envelope.payload.as_slice(),
                )
                .map_err(|source| StoreError::ArchiveDecode {
                    message: source.to_string(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            checkpoint,
            suffix,
            restored_head,
        })
    }

    fn into_store(
        self,
        path: impl Into<PathBuf>,
        expected_head: EntryDigest,
    ) -> Result<Store, StoreError> {
        if self.restored_head != expected_head {
            return Err(StoreError::MirrorRestoreHeadMismatch {
                expected: expected_head,
                restored: self.restored_head,
            });
        }
        Store::import(path, self.checkpoint, self.suffix)
    }
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
                Err(error) => {
                    SemaWriteOutput::missed(ErrorReport::new(ErrorMessage::new(error.to_string())))
                }
            },
            SemaWriteInput::Remove(remove) => {
                let record_identifier = remove.into_payload().record_identifier;
                match self.remove(record_identifier.payload()) {
                    Ok(true) => SemaWriteOutput::removed(RemoveReceipt::new(record_identifier)),
                    Ok(false) => SemaWriteOutput::missed(ErrorReport::new(ErrorMessage::new(
                        "record not found",
                    ))),
                    Err(error) => SemaWriteOutput::missed(ErrorReport::new(ErrorMessage::new(
                        error.to_string(),
                    ))),
                }
            }
            SemaWriteInput::ChangeCertainty(change) => {
                match self.change_certainty(change.into_payload()) {
                    Ok(Some(receipt)) => SemaWriteOutput::certainty_changed(receipt),
                    Ok(None) => SemaWriteOutput::missed(ErrorReport::new(ErrorMessage::new(
                        "record not found",
                    ))),
                    Err(error) => SemaWriteOutput::missed(ErrorReport::new(ErrorMessage::new(
                        error.to_string(),
                    ))),
                }
            }
            SemaWriteInput::BumpImportance(change) => {
                match self.bump_importance(change.into_payload()) {
                    Ok(Some(receipt)) => SemaWriteOutput::importance_bumped(receipt),
                    Ok(None) => SemaWriteOutput::missed(ErrorReport::new(ErrorMessage::new(
                        "record not found",
                    ))),
                    Err(error) => SemaWriteOutput::missed(ErrorReport::new(ErrorMessage::new(
                        error.to_string(),
                    ))),
                }
            }
            SemaWriteInput::ChangeRecord(change) => match self.change_record(change.into_payload())
            {
                Ok(Some(receipt)) => SemaWriteOutput::record_changed(receipt),
                Ok(None) => {
                    SemaWriteOutput::missed(ErrorReport::new(ErrorMessage::new("record not found")))
                }
                Err(error) => {
                    SemaWriteOutput::missed(ErrorReport::new(ErrorMessage::new(error.to_string())))
                }
            },
            SemaWriteInput::RegisterReferent(register) => {
                match self.register_referent(register.into_payload()) {
                    Ok(receipt) => SemaWriteOutput::referent_registered(receipt),
                    Err(error) => SemaWriteOutput::missed(ErrorReport::new(ErrorMessage::new(
                        error.to_string(),
                    ))),
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
                Ok(entries) if !entries.is_empty() => {
                    SemaReadOutput::observed(ObservedRecords::new(RecordSet::new(entries)))
                }
                Ok(_) => SemaReadOutput::missed(ErrorReport::new(ErrorMessage::new(
                    "no matching record",
                ))),
                Err(error) => {
                    SemaReadOutput::missed(ErrorReport::new(ErrorMessage::new(error.to_string())))
                }
            },
            SemaReadInput::PublicTextSearch(search) => match self
                .public_text_search(search.payload())
            {
                Ok(entries) if !entries.is_empty() => SemaReadOutput::public_text_search_results(
                    ObservedRecords::new(RecordSet::new(entries)),
                ),
                Ok(_) => SemaReadOutput::missed(ErrorReport::new(ErrorMessage::new(
                    "no matching record",
                ))),
                Err(error) => {
                    SemaReadOutput::missed(ErrorReport::new(ErrorMessage::new(error.to_string())))
                }
            },
            SemaReadInput::Lookup(lookup) => {
                let record_identifier = lookup.into_payload();
                match self.entry_by_identifier(record_identifier.payload()) {
                    Ok(Some(entry)) => SemaReadOutput::found(FoundRecord {
                        record_identifier,
                        entry,
                    }),
                    Ok(None) => SemaReadOutput::missed(ErrorReport::new(ErrorMessage::new(
                        "record not found",
                    ))),
                    Err(error) => SemaReadOutput::missed(ErrorReport::new(ErrorMessage::new(
                        error.to_string(),
                    ))),
                }
            }
            SemaReadInput::Count(count) => match self.count(count.payload()) {
                Ok(count) => SemaReadOutput::counted(CountedRecords::new(RecordCount::new(count))),
                Err(error) => {
                    SemaReadOutput::missed(ErrorReport::new(ErrorMessage::new(error.to_string())))
                }
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
    /// counter through sema-engine. The store opts into the versioned
    /// commit log with the schema-generated [`RecordFamily`] policy and
    /// registers every schema-declared family through its generated
    /// descriptor, so the log is the authoritative replayable history of
    /// the intent corpus.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let mut database = SemaDatabase::open(
            EngineOpen::new(path.clone(), SPIRIT_SCHEMA_VERSION)
                .with_versioning(RecordFamily::versioning_policy()),
        )?;
        let entries = database.register_table(RecordFamily::records_family())?;
        let referents = database.register_table(RecordFamily::referents_family())?;
        let migrations = database.register_table(RecordFamily::migrations_family())?;
        Ok(Self {
            database: Arc::new(database),
            entries,
            referents,
            migrations,
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

    /// The store-schema version this build writes: the major store-format
    /// generation, bumped only on a breaking store change. Surfaced on the
    /// version report so a caller can see which store generation a running
    /// daemon serves without opening its `*.sema` file.
    pub fn store_schema_version(&self) -> u32 {
        SPIRIT_SCHEMA_VERSION.value()
    }

    /// The engine's content-addressed store-schema hash: the identity of the
    /// registered family set (family names plus per-family schema hashes).
    /// Rendered as lowercase hex so it travels as version-report text.
    pub fn store_schema_hash(&self) -> String {
        self.database.store_schema_hash().to_string()
    }

    /// A shared handle to the underlying versioned engine, for the mirror
    /// shipper. The returned `Arc` clones the SAME engine instance the store
    /// writes through, so the shipper reads the durable outbox the store's
    /// working writes append to and records the server-confirmed head back
    /// into it. Sharing is safe because every working mutator is `&self`
    /// (the engine holds its own internal write lock).
    pub fn engine_handle(&self) -> Arc<SemaDatabase> {
        Arc::clone(&self.database)
    }

    /// Write a checkpoint of the versioned log: the portable restore
    /// artifact a fresh store imports from.
    pub fn checkpoint(&self) -> Result<CheckpointReceipt, StoreError> {
        Ok(self.database.checkpoint()?)
    }

    /// The latest stored checkpoint, content-verified, when one exists.
    pub fn latest_checkpoint(&self) -> Result<Option<Checkpoint>, StoreError> {
        Ok(self.database.latest_checkpoint()?)
    }

    /// The full versioned commit log: the authoritative replayable history
    /// of every durable write since the store opted into versioning.
    pub fn versioned_log(&self) -> Result<Vec<VersionedCommitLogEntry>, StoreError> {
        Ok(self.database.versioned_commit_log()?)
    }

    /// The current versioned-log head: the `EntryDigest` of the last entry in
    /// the replayable history, or `None` when the store has never committed a
    /// versioned entry. This is the content-addressed identity of the local
    /// head `D` the criome gate authorizes BEFORE fan-out — read straight from
    /// the local log after the working commit, never from `ShipOutcome.head`
    /// (which exists only after a ship has happened).
    #[cfg(feature = "mirror-shipper")]
    pub fn versioned_log_head(&self) -> Result<Option<EntryDigest>, StoreError> {
        Ok(self
            .versioned_log()?
            .last()
            .map(VersionedCommitLogEntry::entry_digest))
    }

    /// The versioned-log suffix strictly after `sequence`, in commit order —
    /// the entries an importer ingests on top of a checkpoint.
    pub fn versioned_log_from(
        &self,
        sequence: CommitSequence,
    ) -> Result<Vec<VersionedCommitLogEntry>, StoreError> {
        Ok(self.database.versioned_replay_from_sequence(sequence)?)
    }

    /// Restore a fresh store at `path` from a checkpoint plus versioned-log
    /// suffix through the engine-owned import session. The path must hold no
    /// prior engine history; the imported log and counters land verbatim, so
    /// the restored store is indistinguishable from the original at the
    /// imported range. Family registration happens after the import, against
    /// the restored catalog.
    pub fn import(
        path: impl Into<PathBuf>,
        checkpoint: Checkpoint,
        suffix: Vec<VersionedCommitLogEntry>,
    ) -> Result<Self, StoreError> {
        let path = path.into();
        let mut database = SemaDatabase::open(
            EngineOpen::new(path.clone(), SPIRIT_SCHEMA_VERSION)
                .with_versioning(RecordFamily::versioning_policy()),
        )?;
        let mut session = database.begin_import()?;
        session.ingest_checkpoint(checkpoint)?;
        session.ingest_suffix(suffix);
        session.commit(&StoreFamilyDirectory::from_generated_families())?;
        let entries = database.register_table(RecordFamily::records_family())?;
        let referents = database.register_table(RecordFamily::referents_family())?;
        let migrations = database.register_table(RecordFamily::migrations_family())?;
        Ok(Self {
            database: Arc::new(database),
            entries,
            referents,
            migrations,
            path,
            archive_target: ArchiveDatabaseTarget::Default,
            #[cfg(feature = "testing-trace")]
            trace_log: TraceLog::default(),
        })
    }

    /// Restore from a mirror restore bundle only when the bundle's restored
    /// head is the head the authorized reference announced.
    #[cfg(feature = "mirror-shipper")]
    pub fn import_mirror_restore_bundle(
        path: impl Into<PathBuf>,
        bundle: signal_mirror::RestoreBundle,
        expected_head: EntryDigest,
    ) -> Result<Self, StoreError> {
        MirrorRestoreImport::from_bundle(bundle)?.into_store(path, expected_head)
    }

    /// The typed family directory over this store's registered tables, for
    /// fold materialization (rebuild and import).
    pub fn family_directory(&self) -> StoreFamilyDirectory {
        StoreFamilyDirectory {
            entries: self.entries,
            referents: self.referents,
            migrations: self.migrations,
        }
    }

    /// Record the typed migration marker as an ordinary logged assert, so
    /// the migration itself is part of the replayable history.
    pub fn record_migration(&self, migration: Migration) -> Result<(), StoreError> {
        self.database
            .assert(Assertion::new(self.migrations, migration))?;
        Ok(())
    }

    /// Every recorded migration marker, oldest source version first.
    pub fn migrations(&self) -> Result<Vec<Migration>, StoreError> {
        let mut migrations = self
            .database
            .match_records(QueryPlan::all(self.migrations))?
            .records()
            .to_vec();
        migrations.sort_by_key(|migration| *migration.source_schema_version.payload());
        Ok(migrations)
    }

    /// Import one pre-vetted referent registration with its canonical atom
    /// and aliases unchanged — the migration sibling of [`Self::import_record`].
    pub fn import_referent(&self, referent: StoredReferent) -> Result<(), StoreError> {
        self.database
            .assert(Assertion::new(self.referents, referent))?;
        Ok(())
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
        // The version suffix tracks GUARDIAN_JOURNAL_SCHEMA_VERSION: a
        // journal-schema change (v4: the sema-engine storage-layout-5 break)
        // lands a fresh file rather than reading an incompatible layout. The
        // older file stays on disk untouched, readable by the previous engine.
        self.path.with_file_name(format!("{stem}.guardian.v4.sema"))
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
            let identifier = record.record_identifier.payload().clone();
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
        // Owner bypass: a corpus import carries its own referents. Auto-register
        // any not-yet-registered referent (kebab-validated) directly, without the
        // referent guardian, so the import is self-contained rather than failing
        // on UnregisteredReferent the way the guarded working path would.
        for referent in entry.referents.payload() {
            if self.canonical_referent(referent)?.is_none() {
                self.register_referent_record(referent.clone(), Referents::new(Vec::new()))?;
            }
        }
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
        if let Some(receipt) = self.settled_referent_registration_receipt(&registration)? {
            return Ok(receipt);
        }
        self.register_referent_record(registration.referent, registration.aliases)
    }

    /// The justification-free core of referent registration, shared by the
    /// guarded working path and the owner import bypass. A brand-new referent
    /// name must be lowercase kebab-case; already-registered names are
    /// grandfathered (alias merge), so legacy capitalized referents keep working
    /// while no new non-kebab name can enter the store.
    fn register_referent_record(
        &self,
        referent: Referent,
        aliases: Referents,
    ) -> Result<ReferentRegistrationReceipt, StoreError> {
        let mut record = StoredReferent::new(referent, aliases);
        self.reject_conflicting_referent_names(&record)?;
        if let Some(existing) = self.referent_by_key(record.referent.payload())? {
            record = existing.with_aliases_merged(record.aliases);
            self.database
                .mutate(Mutation::new(self.referents, record.clone()))?;
        } else {
            Self::validate_kebab_referent(record.referent.payload())?;
            self.database
                .assert(Assertion::new(self.referents, record.clone()))?;
        }
        Ok(ReferentRegistrationReceipt::new(record.referent))
    }

    /// A referent name is lowercase kebab-case: ASCII lowercase/digit groups
    /// joined by single hyphens, no leading/trailing/double hyphen.
    fn validate_kebab_referent(name: &str) -> Result<(), StoreError> {
        let well_formed = !name.is_empty()
            && !name.starts_with('-')
            && !name.ends_with('-')
            && !name.contains("--")
            && name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if well_formed {
            Ok(())
        } else {
            Err(StoreError::NonKebabReferent(name.to_string()))
        }
    }

    pub(crate) fn settled_referent_registration_receipt(
        &self,
        registration: &ReferentRegistration,
    ) -> Result<Option<ReferentRegistrationReceipt>, StoreError> {
        Ok(self
            .referents()?
            .into_iter()
            .find(|referent| referent.contains_registration(registration))
            .map(|referent| ReferentRegistrationReceipt::new(referent.referent)))
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
        let record_identifier = duplicate.record_identifier.clone();
        let _importance_receipt = self
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

    fn public_text_search(
        &self,
        search_text: &SearchText,
    ) -> Result<Vec<ObservedRecord>, StoreError> {
        let needle = PublicTextSearchNeedle::new(search_text);
        if needle.is_empty() {
            return Ok(Vec::new());
        }

        let mut scored = self
            .records()?
            .into_iter()
            .filter(|record| record.entry.is_public_active())
            .filter_map(|record| {
                let score = needle.score_entry(&record.entry)?;
                Some((score, record))
            })
            .collect::<Vec<_>>();
        scored.sort_by_key(|(score, record)| {
            std::cmp::Reverse((
                *score,
                record.entry.certainty_rank(),
                record.entry.importance_rank(),
            ))
        });
        Ok(scored
            .into_iter()
            .take(PUBLIC_TEXT_SEARCH_LIMIT)
            .map(|(_score, record)| record.into_observed_record())
            .collect())
    }

    #[cfg(feature = "agent-guardian")]
    pub(crate) fn guardian_records_for_operation(
        &self,
        operation: &GuardianOperation,
    ) -> Result<RecordSet, StoreError> {
        let mut bundle = GuardianRecordBundle::new();
        for candidate in operation.candidate_entries() {
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
            GuardianOperation::ResolveClarification(resolution) => {
                if let Some(current) = self.observed_record_by_identifier(
                    resolution
                        .clarification_record_identifier
                        .payload()
                        .payload(),
                )? {
                    bundle.insert(current);
                }
                for target in resolution.target_clarifications.payload() {
                    if let Some(current) =
                        self.observed_record_by_identifier(target.record_identifier.payload())?
                    {
                        bundle.insert(current.clone());
                        let mut candidate = current.entry;
                        candidate.description = target.description.clone();
                        bundle.extend(self.guardian_records_for_entry(&candidate)?);
                    }
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
                let records = self.observe(collection.record_query.payload())?;
                bundle.extend(RecordSet::new(records));
            }
            GuardianOperation::Record(_) | GuardianOperation::Propose(_) => {}
        }
        Ok(bundle.into_record_set())
    }

    #[cfg(feature = "agent-guardian")]
    fn guardian_records_for_entry(&self, proposed: &Entry) -> Result<RecordSet, StoreError> {
        let proposed = self.canonicalized_entry(proposed.clone())?;
        let mut bundle = GuardianRecordBundle::new();
        for scope in proposed.guardian_domain_scopes().into_payload() {
            bundle.extend(RecordSet::new(
                self.observe(&Query::guardian_domain_scope(scope))?,
            ));
        }
        if !proposed.referents.payload().is_empty() {
            bundle.extend(RecordSet::new(
                self.observe(&Query::guardian_referents(proposed.referents.clone()))?,
            ));
        }
        Ok(bundle.into_record_set())
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
        let record_identifier = duplicate.record_identifier.clone();
        let _importance_receipt = self
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
            Err(engine_error) => Err(StoreError::Database { engine_error }),
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
        Ok(Some(RemoveReceipt::new(record_identifier)))
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
        Ok(Some(RetirementReceipt::new(record_identifier)))
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
        Ok(Some(RecordChangeReceipt::new(record_identifier)))
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
        Ok(Some(ClarificationReceipt::new(record_identifier)))
    }

    pub fn resolve_clarification(
        &self,
        resolution: ClarificationResolution,
    ) -> Result<Option<ClarificationResolutionReceipt>, StoreError> {
        let ClarificationResolution {
            clarification_record_identifier,
            target_clarifications,
            justification: _justification,
        } = resolution;
        let clarification_identifier = clarification_record_identifier.payload().payload().clone();
        if self
            .entry_by_identifier(&clarification_identifier)?
            .is_none()
        {
            return Ok(None);
        }

        let mut snapshots = Vec::new();
        for target in target_clarifications.payload() {
            let identifier_text = target.record_identifier.payload().clone();
            let Some(entry) = self.entry_by_identifier(&identifier_text)? else {
                return Ok(None);
            };
            snapshots.push((
                target.record_identifier.clone(),
                identifier_text,
                entry,
                target.description.clone(),
            ));
        }

        let mut archive = self.open_archive_database()?;
        for (_record_identifier, identifier_text, entry, _description) in &snapshots {
            archive.archive_record(
                StoredRecord::new(identifier_text.clone(), entry.clone()),
                self.archive_identifier(identifier_text),
            )?;
        }

        let mut record_identifiers = Vec::new();
        for (record_identifier, identifier_text, mut entry, description) in snapshots {
            entry.description = description;
            self.database.mutate(Mutation::new(
                self.entries,
                StoredRecord::new(identifier_text, entry),
            ))?;
            record_identifiers.push(record_identifier);
        }

        if !self.remove(&clarification_identifier)? {
            return Ok(None);
        }

        Ok(Some(ClarificationResolutionReceipt {
            clarification_record_identifier: ClarificationRecordIdentifier::new(
                RecordIdentifier::new(clarification_identifier),
            ),
            record_identifiers: RecordIdentifiers::new(record_identifiers),
        }))
    }

    pub fn supersede(
        &self,
        supersession: Supersession,
    ) -> Result<Option<SupersessionReceipt>, StoreError> {
        let retired_identifiers = supersession.retired_identifiers;
        let replacements = supersession.replacements;
        // Snapshot every retired target up front, BEFORE any mutation, so a
        // missing target is a clean no-op rather than a partial write.
        let mut snapshots = Vec::new();
        for identifier in retired_identifiers.payload() {
            let identifier_text = identifier.payload().payload().clone();
            let Some(entry) = self.entry_by_identifier(&identifier_text)? else {
                return Ok(None);
            };
            snapshots.push((identifier_text, entry));
        }
        // Propose every replacement FIRST. If any propose fails here, the
        // retired records are still intact and the caller can safely retry — this
        // ordering is what prevents a partial supersede from destroying intent
        // (a removed retired record with no replacement). A residual leak of an
        // already-committed replacement on a later propose failure is recoverable
        // (an extra record), never a loss; a single sema-engine WriteTransaction
        // spanning the whole supersede is the eventual end state.
        let mut record_identifiers = Vec::new();
        for replacement in replacements.into_payload() {
            let sema_receipt = self.propose(replacement)?;
            record_identifiers.push(sema_receipt.record_identifier);
        }
        // Only once every replacement is committed do we archive and remove the
        // retired set.
        let mut archive = self.open_archive_database()?;
        for (identifier_text, entry) in &snapshots {
            archive.archive_record(
                StoredRecord::new(identifier_text.clone(), entry.clone()),
                self.archive_identifier(identifier_text),
            )?;
        }
        for (identifier_text, _entry) in &snapshots {
            self.remove(identifier_text)?;
        }
        Ok(Some(SupersessionReceipt {
            retired_identifiers,
            record_identifiers: RecordIdentifiers::new(record_identifiers),
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
            hasher.update(record.record_identifier.payload().as_bytes());
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
    pub(super) fn new(record_identifier: String, entry: Entry) -> Self {
        Self {
            record_identifier: RecordIdentifier::new(record_identifier),
            entry,
        }
    }

    fn into_observed_record(self) -> ObservedRecord {
        ObservedRecord {
            record_identifier: self.record_identifier,
            entry: self.entry,
        }
    }

    fn entry(&self) -> Entry {
        self.entry.clone()
    }
}

impl EngineRecord for StoredRecord {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.record_identifier.payload().clone())
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

    fn contains_registration(&self, registration: &ReferentRegistration) -> bool {
        self.matches(&registration.referent)
            && registration
                .aliases
                .payload()
                .iter()
                .all(|alias| self.matches(alias))
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

#[cfg(feature = "agent-guardian")]
pub trait GuardianEntryExt {
    fn guardian_domain_scopes(&self) -> DomainScopes;
}

#[cfg(feature = "agent-guardian")]
impl GuardianEntryExt for Entry {
    fn guardian_domain_scopes(&self) -> DomainScopes {
        DomainScopes::from_domains(&self.domains)
    }
}

#[cfg(feature = "agent-guardian")]
pub trait GuardianQueryExt {
    fn guardian_domain_scope(scope: DomainScope) -> Self;
    fn guardian_referents(referents: Referents) -> Self;
    fn guardian_context(domain_match: DomainMatch, referent_selection: ReferentSelection) -> Self;
}

#[cfg(feature = "agent-guardian")]
impl GuardianQueryExt for Query {
    fn guardian_domain_scope(scope: DomainScope) -> Self {
        Self::guardian_context(
            DomainMatch::full(DomainScopes::new(vec![scope])),
            ReferentSelection::Any,
        )
    }

    fn guardian_referents(referents: Referents) -> Self {
        Self::guardian_context(DomainMatch::Any, ReferentSelection::any_referent(referents))
    }

    fn guardian_context(domain_match: DomainMatch, referent_selection: ReferentSelection) -> Self {
        Self {
            domain_match,
            keyword_match: KeywordMatch::Any,
            text_match: TextMatch::Any,
            referent_selection,
            selected_kind: SelectedKind::new(None),
            privacy_selection: PrivacySelection::Any,
            certainty_selection: CertaintySelection::default_observation_certainty(),
            importance_selection: ImportanceSelection::default_observation_importance(),
        }
    }
}

struct PublicTextSearchNeedle {
    full_text: String,
    tokens: Vec<String>,
}

impl PublicTextSearchNeedle {
    fn new(search_text: &SearchText) -> Self {
        let full_text = search_text.payload().trim().to_lowercase();
        let tokens = full_text
            .split_whitespace()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        Self { full_text, tokens }
    }

    fn is_empty(&self) -> bool {
        self.full_text.is_empty()
    }

    fn score_entry(&self, entry: &Entry) -> Option<u64> {
        let score =
            self.score_description(&entry.description) + self.score_referents(&entry.referents);
        (score > 0).then_some(score)
    }

    fn score_description(&self, description: &Description) -> u64 {
        let haystack = description.payload().to_lowercase();
        let mut score = 0;
        if haystack.contains(&self.full_text) {
            score += 100;
        }
        for token in &self.tokens {
            if haystack.contains(token) {
                score += 10;
            }
        }
        score
    }

    fn score_referents(&self, referents: &Referents) -> u64 {
        let mut score = 0;
        for referent in referents.payload() {
            let candidate = referent.payload().to_lowercase();
            if candidate == self.full_text {
                score += 120;
            } else if candidate.contains(&self.full_text) {
                score += 60;
            }
            for token in &self.tokens {
                if candidate == *token {
                    score += 30;
                } else if candidate.contains(token) {
                    score += 15;
                }
            }
        }
        score
    }
}

pub trait EntryStoreExt {
    fn matches(&self, query: &Query) -> bool;
    fn is_public_active(&self) -> bool;
    fn certainty_rank(&self) -> u64;
    fn importance_rank(&self) -> u64;
}

impl EntryStoreExt for Entry {
    fn matches(&self, query: &Query) -> bool {
        query.matches(self)
    }

    fn is_public_active(&self) -> bool {
        PrivacySelection::default_observation_privacy().matches(&self.privacy)
            && CertaintySelection::default_observation_certainty().matches(&self.certainty)
            && ImportanceSelection::default_observation_importance().matches(&self.importance)
    }

    fn certainty_rank(&self) -> u64 {
        self.certainty.payload().rank()
    }

    fn importance_rank(&self) -> u64 {
        self.importance.payload().rank()
    }
}

pub trait DescriptionStoreExt {
    fn keywords(&self) -> Keywords;
    fn contains_search_text(&self, search_text: &SearchText) -> bool;
    #[cfg(feature = "agent-guardian")]
    fn contains_description_text(&self, other: &Description) -> bool;
}

impl DescriptionStoreExt for Description {
    fn keywords(&self) -> Keywords {
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

    fn contains_search_text(&self, search_text: &SearchText) -> bool {
        self.payload()
            .to_lowercase()
            .contains(&search_text.payload().trim().to_lowercase())
    }

    #[cfg(feature = "agent-guardian")]
    fn contains_description_text(&self, other: &Description) -> bool {
        let other = other.payload().trim();
        !other.is_empty() && self.contains_search_text(&SearchText::new(other))
    }
}

pub trait KeywordStoreExt {
    fn normalized(&self) -> String;
}

impl KeywordStoreExt for Keyword {
    fn normalized(&self) -> String {
        self.payload().trim().to_lowercase()
    }
}

pub trait KeywordsStoreExt {
    fn contains_keyword(&self, expected: &Keyword) -> bool;
    fn contains_any(&self, expected: &Keywords) -> bool;
    fn contains_all(&self, expected: &Keywords) -> bool;
}

impl KeywordsStoreExt for Keywords {
    fn contains_keyword(&self, expected: &Keyword) -> bool {
        let expected = expected.normalized();
        self.payload()
            .iter()
            .any(|keyword| keyword.normalized() == expected)
    }

    fn contains_any(&self, expected: &Keywords) -> bool {
        expected
            .payload()
            .iter()
            .any(|keyword| self.contains_keyword(keyword))
    }

    fn contains_all(&self, expected: &Keywords) -> bool {
        expected
            .payload()
            .iter()
            .all(|keyword| self.contains_keyword(keyword))
    }
}

pub trait QueryStoreExt {
    fn matches(&self, entry: &Entry) -> bool;
}

impl QueryStoreExt for Query {
    fn matches(&self, entry: &Entry) -> bool {
        self.domain_match.matches(&entry.domains)
            && self.keyword_match.matches(&entry.description)
            && self.text_match.matches(&entry.description)
            && self.referent_selection.matches(&entry.referents)
            && self
                .selected_kind
                .payload()
                .as_ref()
                .is_none_or(|kind| &entry.kind == kind)
            && self.privacy_selection.matches(&entry.privacy)
            && self.certainty_selection.matches(&entry.certainty)
            && self.importance_selection.matches(&entry.importance)
    }
}

pub trait ReferentSelectionStoreExt {
    fn matches(&self, entry_referents: &Referents) -> bool;
}

impl ReferentSelectionStoreExt for ReferentSelection {
    fn matches(&self, entry_referents: &Referents) -> bool {
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

pub trait KeywordMatchStoreExt {
    fn matches(&self, description: &Description) -> bool;
}

impl KeywordMatchStoreExt for KeywordMatch {
    fn matches(&self, description: &Description) -> bool {
        match self {
            Self::Any => true,
            Self::AnyKeyword(expected) => description.keywords().contains_any(expected.payload()),
            Self::AllKeywords(expected) => description.keywords().contains_all(expected.payload()),
        }
    }
}

pub trait TextMatchStoreExt {
    fn matches(&self, description: &Description) -> bool;
}

impl TextMatchStoreExt for TextMatch {
    fn matches(&self, description: &Description) -> bool {
        match self {
            Self::Any => true,
            Self::ContainsText(search_text) => {
                description.contains_search_text(search_text.payload())
            }
        }
    }
}

pub trait PrivacySelectionStoreExt {
    fn default_observation_privacy() -> Self;
    fn matches(&self, privacy: &Privacy) -> bool;
}

impl PrivacySelectionStoreExt for PrivacySelection {
    fn default_observation_privacy() -> Self {
        Self::exact(Privacy::new(Magnitude::Zero))
    }

    fn matches(&self, privacy: &Privacy) -> bool {
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

pub trait CertaintySelectionStoreExt {
    fn default_observation_certainty() -> Self;
    fn removal_candidate_certainty() -> Self;
    fn matches(&self, certainty: &Certainty) -> bool;
}

impl CertaintySelectionStoreExt for CertaintySelection {
    fn default_observation_certainty() -> Self {
        Self::at_least_certainty(Certainty::new(Magnitude::Minimum))
    }

    fn removal_candidate_certainty() -> Self {
        Self::exact_certainty(Certainty::new(Magnitude::Zero))
    }

    fn matches(&self, certainty: &Certainty) -> bool {
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

pub trait ImportanceSelectionStoreExt {
    fn default_observation_importance() -> Self;
    fn matches(&self, importance: &Importance) -> bool;
}

impl ImportanceSelectionStoreExt for ImportanceSelection {
    fn default_observation_importance() -> Self {
        Self::Any
    }

    fn matches(&self, importance: &Importance) -> bool {
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

pub trait ImportanceStoreExt {
    fn next(&self) -> Self;
}

impl ImportanceStoreExt for Importance {
    fn next(&self) -> Self {
        Self::new(self.payload().next())
    }
}

pub trait MagnitudeStoreExt {
    fn rank(&self) -> u64;
    fn next(&self) -> Self;
}

impl MagnitudeStoreExt for Magnitude {
    fn rank(&self) -> u64 {
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

    fn next(&self) -> Self {
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
