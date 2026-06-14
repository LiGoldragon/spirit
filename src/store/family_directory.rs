//! Family-directory plumbing: the component's typed knowledge of where each
//! schema-declared record family materializes.
//!
//! The fold/import surface hands [`StoreFamilyDirectory`] one canonical-view
//! row at a time and it lands the row in the right typed table. Dispatch is on
//! the generated per-family schema hash, so no family name is ever hand-typed
//! here. The directory is data-bearing: it cannot materialize a row without
//! the three [`TableReference`]s it holds.

use sema_engine::{FamilyDirectory, RecordKey, RowMaterializer, SchemaHash, TableReference};

use crate::schema::sema::{Migration, RecordFamily, StoredRecord, StoredReferent, family_identity};

/// The component's typed knowledge of where each schema-declared record
/// family materializes: the fold/import surface hands this directory one
/// canonical-view row at a time and it lands the row in the right typed
/// table. Dispatch is on the generated per-family schema hash, so no family
/// name is ever hand-typed here.
pub struct StoreFamilyDirectory {
    pub(super) entries: TableReference<StoredRecord>,
    pub(super) referents: TableReference<StoredReferent>,
    pub(super) migrations: TableReference<Migration>,
}

impl StoreFamilyDirectory {
    /// The directory derived purely from the generated descriptors, for
    /// import into a virgin store whose tables are not yet registered.
    pub(super) fn from_generated_families() -> Self {
        Self {
            entries: TableReference::new(*RecordFamily::records_family().name()),
            referents: TableReference::new(*RecordFamily::referents_family().name()),
            migrations: TableReference::new(*RecordFamily::migrations_family().name()),
        }
    }
}

impl FamilyDirectory for StoreFamilyDirectory {
    fn materialize(&self, row: RowMaterializer<'_>) -> sema_engine::Result<()> {
        let schema_hash = row.family().schema_hash();
        if schema_hash == SchemaHash::new(family_identity::RECORDS_FAMILY) {
            row.apply(self.entries)
        } else if schema_hash == SchemaHash::new(family_identity::REFERENTS_FAMILY) {
            row.apply(self.referents)
        } else if schema_hash == SchemaHash::new(family_identity::MIGRATIONS_FAMILY) {
            row.apply(self.migrations)
        } else {
            Err(sema_engine::Error::FamilyUnknown {
                family: row.family().family().as_str().to_owned(),
            })
        }
    }
}

impl Migration {
    /// The marker row's stable key in the migrations family, derived from
    /// the typed source schema version — one row per migrated-from version,
    /// so a repeated fold from the same source lands on the same key. The
    /// single named home of the marker-key format: typed in, string out.
    fn marker_key(&self) -> RecordKey {
        RecordKey::new(format!(
            "from-schema-{}",
            self.source_schema_version.payload()
        ))
    }
}

impl sema_engine::EngineRecord for Migration {
    fn record_key(&self) -> RecordKey {
        self.marker_key()
    }
}
