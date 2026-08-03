//! One-way, offline projection from Spirit store schema 13 to schema 14.
//!
//! The migration reads the frozen v13 materialized tables, retains each
//! record's identifier, domains, kind, description, and importance, and
//! creates a fresh v14 log containing only those projected records plus one
//! v13-to-v14 receipt. The v13 referent catalogue, prior migration receipts,
//! log, and checkpoints are deliberately not replayed.
//!
//! Before either current store is exposed, the complete live and lifecycle
//! archive projections are built and reopened for validation. Exact v13 live,
//! archive, and guardian-v6 bytes are retained only as byte-for-byte copies inside a
//! private rollback directory beside the live store.

/// Frozen schema-version-13 readers. This is the only legacy decoder in the
/// release and is compiled only into the offline migration feature.
pub mod v13;

use std::{
    fs,
    os::unix::{fs::DirBuilderExt, fs::PermissionsExt},
    path::{Path, PathBuf},
};

use nota::{NotaDecode, NotaDecodeError, NotaEncode, NotaSource};
use thiserror::Error;

use crate::{
    Store, StoreError,
    schema::{
        sema::{MigratedRecordCount, Migration, SourceSchemaVersion, StoredRecord},
        signal::{Description, Domains, Entry, Importance, Kind},
    },
    store::ArchiveDatabase,
};

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct StoreMigrationRequest {
    database_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct StoreMigrationCompleted {
    record_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub enum StoreMigrationOutput {
    Current(StoreMigrationCompleted),
    Migrated(StoreMigrationCompleted),
}

#[derive(Debug, Error)]
pub enum StoreMigrationError {
    #[error("frozen v13 spirit store: {0}")]
    FrozenV13(#[from] v13::ReaderError),
    #[error("current spirit store: {0}")]
    Store(#[from] StoreError),
    #[error("project retained v13 record fields: {0}")]
    Projection(#[from] NotaDecodeError),
    #[error("store migration io: {0}")]
    Io(#[from] std::io::Error),
    #[error("rollback bundle {path} is incomplete: missing {missing}")]
    IncompleteRollbackBundle {
        path: PathBuf,
        missing: &'static str,
    },
    #[error("migrated store validation failed: {0}")]
    Validation(String),
}

pub struct StoreMigration {
    request: StoreMigrationRequest,
}

enum ArchiveSource {
    Absent,
    VersionThirteen(Vec<v13::StoredRecord>),
    Current(Vec<StoredRecord>),
}

impl StoreMigrationRequest {
    pub fn new(database_path: impl Into<String>) -> Self {
        Self {
            database_path: database_path.into(),
        }
    }

    pub fn database_path(&self) -> &str {
        &self.database_path
    }
}

impl StoreMigrationCompleted {
    pub fn record_count(&self) -> u64 {
        self.record_count
    }
}

impl StoreMigrationOutput {
    pub fn current(completed: StoreMigrationCompleted) -> Self {
        Self::Current(completed)
    }

    pub fn migrated(completed: StoreMigrationCompleted) -> Self {
        Self::Migrated(completed)
    }
}

impl StoreMigration {
    pub fn new(request: StoreMigrationRequest) -> Self {
        Self { request }
    }

    pub fn run(&self) -> Result<StoreMigrationOutput, StoreMigrationError> {
        let database_path = PathBuf::from(self.request.database_path());
        if !database_path.exists() {
            return Ok(StoreMigrationOutput::current(StoreMigrationCompleted {
                record_count: 0,
            }));
        }

        // Snapshot before any engine open: a read-oriented open may still
        // update storage bookkeeping, so copying later would not preserve the
        // exact quiesced v13 bytes.
        let created_rollback = self.stage_rollback_bundle(&database_path)?;
        if let Ok(store) = Store::open(&database_path) {
            if created_rollback {
                Self::remove_new_rollback_bundle(&database_path)?;
            }
            return Ok(StoreMigrationOutput::current(StoreMigrationCompleted {
                record_count: store.len() as u64,
            }));
        }

        match self.migrate_version_thirteen(database_path.clone()) {
            Ok(output) => Ok(output),
            Err(error @ StoreMigrationError::FrozenV13(_)) if created_rollback => {
                Self::remove_new_rollback_bundle(&database_path)?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    fn migrate_version_thirteen(
        &self,
        database_path: PathBuf,
    ) -> Result<StoreMigrationOutput, StoreMigrationError> {
        let source = v13::LiveReader::open(&database_path)?;
        let inventory = source.enumerate()?;
        drop(source);

        // Enumeration validates every v13 family, including the two families
        // intentionally discarded by the projection.
        let projected_records = inventory
            .records
            .into_iter()
            .map(Self::project_record)
            .collect::<Result<Vec<_>, _>>()?;
        let archive_path = Self::archive_sibling_path(&database_path);
        let archive_source = Self::read_archive_source(&archive_path)?;
        let projected_archive = match &archive_source {
            ArchiveSource::Absent => None,
            ArchiveSource::VersionThirteen(records) => Some(
                records
                    .iter()
                    .cloned()
                    .map(Self::project_record)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ArchiveSource::Current(records) => Some(records.clone()),
        };

        self.validate_rollback_bundle(&database_path, &archive_source)?;
        Self::sweep_stale_temporaries(&database_path)?;
        Self::sweep_stale_temporaries(&archive_path)?;

        let live_temporary = Self::temporary_path(&database_path);
        let archive_temporary = Self::temporary_path(&archive_path);
        Self::build_live_projection(&live_temporary, &projected_records)?;
        if let Some(records) = &projected_archive {
            Self::build_archive_projection(&archive_temporary, records)?;
        }

        // Reopen and compare retained substance before exposing either file.
        Self::validate_live_projection(&live_temporary, &projected_records)?;
        if let Some(records) = &projected_archive {
            Self::validate_archive_projection(&archive_temporary, records)?;
        }

        // Archive first makes a crash between the two renames recoverable: a
        // rerun can read the already-current archive while the live v13 source
        // and its private rollback links remain authoritative.
        if projected_archive.is_some() {
            fs::rename(&archive_temporary, &archive_path)?;
        }
        fs::rename(&live_temporary, &database_path)?;

        Ok(StoreMigrationOutput::migrated(StoreMigrationCompleted {
            record_count: projected_records.len() as u64,
        }))
    }

    fn project_record(record: v13::StoredRecord) -> Result<StoredRecord, StoreMigrationError> {
        // The v13 and v14 domain/kind/magnitude vocabularies share the exact
        // canonical NOTA spellings. Decode each retained field independently;
        // never decode the legacy seven-field Entry into a current Entry.
        let domains_text = record.entry.domains.to_nota();
        let kind_text = record.entry.kind.to_nota();
        let description_text = record.entry.description.to_nota();
        let importance_text = record.entry.importance.to_nota();
        let domains: Domains = NotaSource::new(&domains_text).parse()?;
        let kind: Kind = NotaSource::new(&kind_text).parse()?;
        let description: Description = NotaSource::new(&description_text).parse()?;
        let importance: Importance = NotaSource::new(&importance_text).parse()?;

        Ok(StoredRecord {
            record_identifier: crate::schema::signal::RecordIdentifier::new(
                record.record_identifier.into_payload(),
            ),
            entry: Entry {
                domains,
                kind,
                description,
                importance,
            },
        })
    }

    fn build_live_projection(
        path: &Path,
        records: &[StoredRecord],
    ) -> Result<(), StoreMigrationError> {
        let store = Store::open(path)?;
        for record in records {
            store.import_record(
                record.record_identifier.payload().clone(),
                record.entry.clone(),
            )?;
        }
        store.record_migration(Migration {
            source_schema_version: SourceSchemaVersion::new(13),
            migrated_record_count: MigratedRecordCount::new(records.len() as u64),
        })?;
        drop(store);
        Ok(())
    }

    fn build_archive_projection(
        path: &Path,
        records: &[StoredRecord],
    ) -> Result<(), StoreMigrationError> {
        let mut archive = ArchiveDatabase::open(path)?;
        for record in records {
            archive.import_archived_record(record.clone())?;
        }
        drop(archive);
        Ok(())
    }

    fn validate_live_projection(
        path: &Path,
        records: &[StoredRecord],
    ) -> Result<(), StoreMigrationError> {
        let store = Store::open(path)?;
        if store.len() != records.len() {
            return Err(StoreMigrationError::Validation(format!(
                "expected {} projected live records, found {}",
                records.len(),
                store.len()
            )));
        }
        for record in records {
            let found = store.entry_by_identifier(record.record_identifier.payload())?;
            if found.as_ref() != Some(&record.entry) {
                return Err(StoreMigrationError::Validation(format!(
                    "projected live record {} differs from retained v13 fields",
                    record.record_identifier.payload()
                )));
            }
        }
        let migrations = store.migrations()?;
        if migrations.len() != 1
            || *migrations[0].source_schema_version.payload() != 13
            || *migrations[0].migrated_record_count.payload() != records.len() as u64
        {
            return Err(StoreMigrationError::Validation(String::from(
                "fresh v14 history does not contain exactly one v13 projection receipt",
            )));
        }
        Ok(())
    }

    fn validate_archive_projection(
        path: &Path,
        expected: &[StoredRecord],
    ) -> Result<(), StoreMigrationError> {
        let archive = ArchiveDatabase::open(path)?;
        let found = archive.migration_records()?;
        if found != Self::sorted_records(expected.to_vec()) {
            return Err(StoreMigrationError::Validation(String::from(
                "projected lifecycle archive differs from retained v13 fields",
            )));
        }
        Ok(())
    }

    fn read_archive_source(path: &Path) -> Result<ArchiveSource, StoreMigrationError> {
        if !path.exists() {
            return Ok(ArchiveSource::Absent);
        }
        if let Ok(reader) = v13::ArchiveReader::open(path) {
            return Ok(ArchiveSource::VersionThirteen(reader.records()?));
        }
        // This is the normal crash-recovery state after the archive rename and
        // before the live rename. No other current archive is accepted while a
        // v13 live source is present unless an earlier rollback bundle proves
        // where the original archive bytes survive.
        let archive = ArchiveDatabase::open(path)?;
        Ok(ArchiveSource::Current(archive.migration_records()?))
    }

    fn stage_rollback_bundle(&self, live_path: &Path) -> Result<bool, StoreMigrationError> {
        let bundle = Self::rollback_bundle_path(live_path);
        let created = !bundle.exists();
        if created {
            fs::DirBuilder::new().mode(0o700).create(&bundle)?;
        }
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700))?;

        Self::ensure_snapshot_copy(live_path, &bundle.join("live.v13.sema"))?;
        let archive = Self::archive_sibling_path(live_path);
        if archive.exists() && !bundle.join("archive.v13.sema").exists() {
            fs::copy(&archive, bundle.join("archive.v13.sema"))?;
        }
        let journal = Self::guardian_v6_path(live_path);
        if journal.exists() && !bundle.join("guardian.v6.sema").exists() {
            fs::copy(&journal, bundle.join("guardian.v6.sema"))?;
        }
        Ok(created)
    }

    fn validate_rollback_bundle(
        &self,
        live_path: &Path,
        archive_source: &ArchiveSource,
    ) -> Result<(), StoreMigrationError> {
        let bundle = Self::rollback_bundle_path(live_path);
        if !bundle.join("live.v13.sema").exists() {
            return Err(StoreMigrationError::IncompleteRollbackBundle {
                path: bundle,
                missing: "live.v13.sema",
            });
        }
        match archive_source {
            ArchiveSource::Absent => {}
            ArchiveSource::VersionThirteen(_) | ArchiveSource::Current(_) => {
                let backup = bundle.join("archive.v13.sema");
                if !backup.exists() {
                    return Err(StoreMigrationError::IncompleteRollbackBundle {
                        path: bundle,
                        missing: "archive.v13.sema",
                    });
                }
            }
        }
        Ok(())
    }

    fn ensure_snapshot_copy(source: &Path, backup: &Path) -> Result<(), StoreMigrationError> {
        if backup.exists() {
            return Ok(());
        }
        fs::copy(source, backup)?;
        Ok(())
    }

    fn remove_new_rollback_bundle(live_path: &Path) -> Result<(), StoreMigrationError> {
        let bundle = Self::rollback_bundle_path(live_path);
        for name in ["live.v13.sema", "archive.v13.sema", "guardian.v6.sema"] {
            let path = bundle.join(name);
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        if bundle.exists() {
            fs::remove_dir(bundle)?;
        }
        Ok(())
    }

    fn sorted_records(mut records: Vec<StoredRecord>) -> Vec<StoredRecord> {
        records.sort_by(|left, right| {
            left.record_identifier
                .payload()
                .cmp(right.record_identifier.payload())
        });
        records
    }

    fn archive_sibling_path(database_path: &Path) -> PathBuf {
        let stem = Self::file_stem(database_path);
        database_path.with_file_name(format!("{stem}.archive.sema"))
    }

    fn guardian_v6_path(database_path: &Path) -> PathBuf {
        let stem = Self::file_stem(database_path);
        database_path.with_file_name(format!("{stem}.guardian.v6.sema"))
    }

    fn rollback_bundle_path(database_path: &Path) -> PathBuf {
        let stem = Self::file_stem(database_path);
        database_path.with_file_name(format!("{stem}.schema-13-rollback"))
    }

    fn temporary_path(database_path: &Path) -> PathBuf {
        database_path.with_extension(format!("schema-14-migrating-{}.sema", std::process::id()))
    }

    fn temporary_name_prefix(database_path: &Path) -> String {
        format!("{}.schema-14-migrating-", Self::file_stem(database_path))
    }

    fn sweep_stale_temporaries(database_path: &Path) -> Result<(), StoreMigrationError> {
        let directory = database_path.parent().unwrap_or_else(|| Path::new("."));
        let prefix = Self::temporary_name_prefix(database_path);
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) && name.ends_with(".sema") {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    fn file_stem(path: &Path) -> String {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("spirit"))
    }
}
