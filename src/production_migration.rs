use std::{
    fs,
    path::{Path, PathBuf},
};

use nota_next::{NotaDecode, NotaEncode};
use sema_engine::{
    Engine as SemaDatabase, EngineOpen, EngineRecord, QueryPlan, RecordKey, SchemaVersion,
    StorageKernelError, TableDescriptor, TableName, TableReference,
};
use thiserror::Error;

use crate::{
    Store, StoreError,
    schema::signal::{Certainty, Description, Entry, Kind, Magnitude, Privacy, Topics, Weight},
};

const PRODUCTION_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(5);
const SPIRIT_STORE_V1_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(1);
const SPIRIT_STORE_V2_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(2);
const RECORDS_TABLE: TableName = TableName::new("records");

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct ProductionMigrationRequest {
    source_database_path: String,
    target_database_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct ProductionMigrationCompleted {
    record_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub enum ProductionMigrationOutput {
    Completed(ProductionMigrationCompleted),
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct SpiritStoreUpgradeRequest {
    database_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct SpiritStoreUpgradeCompleted {
    record_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub enum SpiritStoreUpgradeOutput {
    Current(SpiritStoreUpgradeCompleted),
    Upgraded(SpiritStoreUpgradeCompleted),
}

#[derive(Debug, Error)]
pub enum ProductionMigrationError {
    #[error("production database: {0}")]
    ProductionDatabase(#[from] sema_engine::Error),
    #[error("new spirit store: {0}")]
    Store(#[from] StoreError),
    #[error("store upgrade io: {0}")]
    Io(#[from] std::io::Error),
}

struct ProductionDatabase {
    database: SemaDatabase,
    records: TableReference<ProductionStoredRecord>,
}

struct SpiritStoreV1Database {
    database: SemaDatabase,
    records: TableReference<SpiritStoreV1Record>,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct ProductionStoredRecord {
    identifier: signal_spirit::RecordIdentifier,
    entry: ProductionStampedEntry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct ProductionStampedEntry {
    entry: signal_spirit::Entry,
    date: signal_spirit::Date,
    time: signal_spirit::Time,
}

pub struct ProductionMigration {
    request: ProductionMigrationRequest,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV1Record {
    record_identifier: String,
    entry: SpiritStoreV1Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV1Entry {
    topics: Topics,
    kind: Kind,
    description: Description,
    magnitude: Magnitude,
    privacy: Privacy,
}

pub struct SpiritStoreUpgrade {
    request: SpiritStoreUpgradeRequest,
}

impl ProductionMigrationRequest {
    pub fn new(
        source_database_path: impl Into<String>,
        target_database_path: impl Into<String>,
    ) -> Self {
        Self {
            source_database_path: source_database_path.into(),
            target_database_path: target_database_path.into(),
        }
    }

    pub fn source_database_path(&self) -> &str {
        &self.source_database_path
    }

    pub fn target_database_path(&self) -> &str {
        &self.target_database_path
    }
}

impl ProductionMigrationCompleted {
    pub fn record_count(&self) -> u64 {
        self.record_count
    }
}

impl ProductionMigrationOutput {
    pub fn completed(completed: ProductionMigrationCompleted) -> Self {
        Self::Completed(completed)
    }
}

impl SpiritStoreUpgradeRequest {
    pub fn new(database_path: impl Into<String>) -> Self {
        Self {
            database_path: database_path.into(),
        }
    }

    pub fn database_path(&self) -> &str {
        &self.database_path
    }
}

impl SpiritStoreUpgradeCompleted {
    pub fn record_count(&self) -> u64 {
        self.record_count
    }
}

impl SpiritStoreUpgradeOutput {
    pub fn current(completed: SpiritStoreUpgradeCompleted) -> Self {
        Self::Current(completed)
    }

    pub fn upgraded(completed: SpiritStoreUpgradeCompleted) -> Self {
        Self::Upgraded(completed)
    }
}

impl ProductionMigration {
    pub fn new(request: ProductionMigrationRequest) -> Self {
        Self { request }
    }

    pub fn run(&self) -> Result<ProductionMigrationCompleted, ProductionMigrationError> {
        let production_database =
            ProductionDatabase::open(Path::new(self.request.source_database_path()))?;
        let target_store = Store::open(PathBuf::from(self.request.target_database_path()))?;
        let records = production_database.records()?;
        for record in records.iter().cloned() {
            target_store.import_record(record.record_identifier(), record.into_new_entry())?;
        }
        Ok(ProductionMigrationCompleted {
            record_count: records.len() as u64,
        })
    }
}

impl SpiritStoreUpgrade {
    pub fn new(request: SpiritStoreUpgradeRequest) -> Self {
        Self { request }
    }

    pub fn run(&self) -> Result<SpiritStoreUpgradeOutput, ProductionMigrationError> {
        let database_path = PathBuf::from(self.request.database_path());
        if !database_path.exists() {
            return Ok(SpiritStoreUpgradeOutput::current(
                SpiritStoreUpgradeCompleted { record_count: 0 },
            ));
        }
        match Store::open(&database_path) {
            Ok(store) => {
                return Ok(SpiritStoreUpgradeOutput::current(
                    SpiritStoreUpgradeCompleted {
                        record_count: store.len() as u64,
                    },
                ));
            }
            Err(StoreError::Database(sema_engine::Error::Sema(
                StorageKernelError::SchemaVersionMismatch { expected, found },
            ))) if expected == SPIRIT_STORE_V2_SCHEMA_VERSION
                && found == SPIRIT_STORE_V1_SCHEMA_VERSION => {}
            Err(error) => return Err(error.into()),
        }

        let old_database = SpiritStoreV1Database::open(&database_path)?;
        let records = old_database.records()?;
        let temporary_path = Self::temporary_path(&database_path);
        if temporary_path.exists() {
            fs::remove_file(&temporary_path)?;
        }
        let target_store = Store::open(&temporary_path)?;
        for record in records.iter().cloned() {
            target_store
                .import_record(record.record_identifier.clone(), record.into_new_entry())?;
        }
        let backup_path = Self::backup_path(&database_path);
        fs::rename(&database_path, backup_path)?;
        fs::rename(temporary_path, database_path)?;
        Ok(SpiritStoreUpgradeOutput::upgraded(
            SpiritStoreUpgradeCompleted {
                record_count: records.len() as u64,
            },
        ))
    }

    fn temporary_path(database_path: &Path) -> PathBuf {
        database_path.with_extension(format!("schema-2-migrating-{}.sema", std::process::id()))
    }

    fn backup_path(database_path: &Path) -> PathBuf {
        for suffix in 0.. {
            let candidate = database_path.with_extension(format!("schema-1-backup-{suffix}.sema"));
            if !candidate.exists() {
                return candidate;
            }
        }
        unreachable!("unbounded suffix search returns")
    }
}

impl ProductionDatabase {
    fn open(path: &Path) -> Result<Self, ProductionMigrationError> {
        let mut database = SemaDatabase::open(EngineOpen::new(path, PRODUCTION_SCHEMA_VERSION))?;
        let records = database.register_table(TableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<ProductionStoredRecord>, ProductionMigrationError> {
        Ok(self
            .database
            .match_records(QueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV1Database {
    fn open(path: &Path) -> Result<Self, ProductionMigrationError> {
        let mut database =
            SemaDatabase::open(EngineOpen::new(path, SPIRIT_STORE_V1_SCHEMA_VERSION))?;
        let records = database.register_table(TableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV1Record>, ProductionMigrationError> {
        Ok(self
            .database
            .match_records(QueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl ProductionStoredRecord {
    fn record_identifier(&self) -> String {
        self.identifier.code()
    }

    fn into_new_entry(self) -> Entry {
        self.entry.into_new_entry()
    }
}

impl EngineRecord for ProductionStoredRecord {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.identifier.code())
    }
}

impl SpiritStoreV1Record {
    fn into_new_entry(self) -> Entry {
        self.entry.into_new_entry()
    }
}

impl EngineRecord for SpiritStoreV1Record {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.record_identifier.clone())
    }
}

impl SpiritStoreV1Entry {
    fn into_new_entry(self) -> Entry {
        Entry {
            topics: self.topics,
            kind: self.kind,
            description: self.description,
            certainty: Certainty::new(self.magnitude),
            weight: Weight::new(Magnitude::Minimum),
            privacy: self.privacy,
        }
    }
}

impl ProductionStampedEntry {
    fn into_new_entry(self) -> Entry {
        Entry {
            topics: Topics::from_strings(
                self.entry
                    .topics
                    .as_slice()
                    .iter()
                    .map(|topic| topic.as_str().to_owned())
                    .collect(),
            ),
            kind: Self::kind_from(self.entry.kind),
            description: Description::new(self.entry.description.as_str().to_owned()),
            certainty: Certainty::new(Self::magnitude_from(self.entry.certainty)),
            weight: Weight::new(Magnitude::Minimum),
            privacy: Privacy::new(Self::magnitude_from(self.entry.privacy)),
        }
    }

    fn kind_from(value: signal_spirit::Kind) -> Kind {
        match value {
            signal_spirit::Kind::Decision => Kind::Decision,
            signal_spirit::Kind::Principle => Kind::Principle,
            signal_spirit::Kind::Correction => Kind::Correction,
            signal_spirit::Kind::Clarification => Kind::Clarification,
            signal_spirit::Kind::Constraint => Kind::Constraint,
        }
    }

    fn magnitude_from(value: signal_spirit::Magnitude) -> Magnitude {
        match value {
            signal_spirit::Magnitude::Zero => Magnitude::Zero,
            signal_spirit::Magnitude::Minimum => Magnitude::Minimum,
            signal_spirit::Magnitude::VeryLow => Magnitude::VeryLow,
            signal_spirit::Magnitude::Low => Magnitude::Low,
            signal_spirit::Magnitude::Medium => Magnitude::Medium,
            signal_spirit::Magnitude::High => Magnitude::High,
            signal_spirit::Magnitude::VeryHigh => Magnitude::VeryHigh,
            signal_spirit::Magnitude::Maximum => Magnitude::Maximum,
        }
    }
}
