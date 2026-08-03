//! The SEPARATE archive database: a sema-engine keyed table over its own
//! `*.sema` file, distinct from the live intent log.
//!
//! Explicit lifecycle operations open one of these on demand at the
//! owner-configured [`ArchiveDatabaseTarget`](crate::schema::meta_signal::ArchiveDatabaseTarget)
//! and capture the prior record before mutating or retracting it. The archive
//! owns no relationship to the live `Store` database beyond preserving those
//! lifecycle records.

use std::path::PathBuf;

#[cfg(feature = "production-migration")]
use sema_engine::QueryPlan;
use sema_engine::{Assertion, Engine as SemaDatabase, EngineOpen, TableReference};

use crate::schema::sema::{RecordFamily, StoredRecord};

use super::{SPIRIT_SCHEMA_VERSION, StoreError};

pub(crate) struct ArchiveDatabase {
    database: SemaDatabase,
    entries: TableReference<StoredRecord>,
}

impl ArchiveDatabase {
    /// Open the archive at the same schema version as the live store,
    /// registered through the same generated records-family descriptor. The
    /// archive stays unversioned (no `VersioningPolicy`): it is a derived
    /// holding pen for records the live log let go, not an authoritative
    /// history of its own.
    pub(crate) fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let mut database = SemaDatabase::open(EngineOpen::new(path.into(), SPIRIT_SCHEMA_VERSION))?;
        let entries = database.register_table(RecordFamily::records_family())?;
        Ok(Self { database, entries })
    }

    /// Durably assert an archived copy of one live `Entry` into the separate
    /// archive database under a versioned archive key, so repeated clarification
    /// and retirement of the same live identifier preserve every prior state.
    pub(super) fn archive_record(
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

    /// Durably import one already-keyed archived record verbatim — the
    /// archive-migration path. Live archiving re-keys through
    /// [`Self::archive_record`].
    #[cfg(feature = "production-migration")]
    pub(crate) fn import_archived_record(
        &mut self,
        record: StoredRecord,
    ) -> Result<(), StoreError> {
        self.database.assert(Assertion::new(self.entries, record))?;
        Ok(())
    }

    /// Enumerate projected archive rows for offline migration validation and
    /// crash recovery. This is not a runtime archive-query surface.
    #[cfg(feature = "production-migration")]
    pub(crate) fn migration_records(&self) -> Result<Vec<StoredRecord>, StoreError> {
        let mut records = self
            .database
            .match_records(QueryPlan::all(self.entries))?
            .records()
            .to_vec();
        records.sort_by(|left, right| {
            left.record_identifier
                .payload()
                .cmp(right.record_identifier.payload())
        });
        Ok(records)
    }
}
