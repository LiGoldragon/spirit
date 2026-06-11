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
    schema::signal::{
        Certainty, Description, Domains, Entry, Importance, Kind, Magnitude, Privacy, Referents,
    },
};

const PRODUCTION_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(5);
const SPIRIT_STORE_V1_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(1);
const SPIRIT_STORE_V2_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(2);
const SPIRIT_STORE_V3_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(3);
const SPIRIT_STORE_V4_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(4);
const SPIRIT_STORE_V5_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(5);
const SPIRIT_STORE_V6_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(6);
const SPIRIT_STORE_CURRENT_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(7);
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

pub struct ProductionMigration {
    request: ProductionMigrationRequest,
}

pub struct SpiritStoreUpgrade {
    request: SpiritStoreUpgradeRequest,
}

struct ProductionDatabase {
    database: SemaDatabase,
    records: TableReference<ProductionStoredRecord>,
}

struct SpiritStoreV1Database {
    database: SemaDatabase,
    records: TableReference<SpiritStoreV1Record>,
}

struct SpiritStoreV2Database {
    database: SemaDatabase,
    records: TableReference<SpiritStoreV2Record>,
}

struct SpiritStoreV4Database {
    database: SemaDatabase,
    records: TableReference<SpiritStoreV4Record>,
}

struct SpiritStoreV5Database {
    database: SemaDatabase,
    records: TableReference<SpiritStoreV5Record>,
}

struct SpiritStoreV6Database {
    database: SemaDatabase,
    records: TableReference<SpiritStoreV6Record>,
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

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV1Record {
    record_identifier: String,
    entry: SpiritStoreV1Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV1Entry {
    categories: LegacyTextCategories,
    kind: Kind,
    description: Description,
    magnitude: Magnitude,
    privacy: Privacy,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV2Record {
    record_identifier: String,
    entry: SpiritStoreV2Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV2Entry {
    categories: LegacyTextCategories,
    kind: Kind,
    description: Description,
    certainty: Certainty,
    importance: Importance,
    privacy: Privacy,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV4Record {
    record_identifier: String,
    entry: SpiritStoreV4Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV4Entry {
    categories: LegacyTextCategories,
    kind: Kind,
    description: Description,
    certainty: Certainty,
    importance: Importance,
    weight: LegacyWeight,
    privacy: Privacy,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV5Record {
    record_identifier: String,
    entry: SpiritStoreV5Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV5Entry {
    categories: LegacyCategories,
    kind: Kind,
    description: Description,
    certainty: Certainty,
    importance: Importance,
    weight: LegacyWeight,
    privacy: Privacy,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV6Record {
    record_identifier: String,
    entry: SpiritStoreV6Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV6Entry {
    categories: LegacyCategories,
    kind: Kind,
    description: Description,
    certainty: Certainty,
    importance: Importance,
    privacy: Privacy,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct LegacyTextCategory(String);

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct LegacyTextCategories(Vec<LegacyTextCategory>);

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyCategory {
    Being,
    Knowing,
    Meaning,
    Making,
    Relating,
    Governing,
    Caring,
    Sustaining,
    Dwelling,
    Moving,
    Valuing,
    Expressing,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct LegacyCategories(Vec<LegacyCategory>);

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct LegacyWeight(u64);

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
            Ok(store) => Ok(SpiritStoreUpgradeOutput::current(
                SpiritStoreUpgradeCompleted {
                    record_count: store.len() as u64,
                },
            )),
            Err(StoreError::Database(sema_engine::Error::Sema(
                StorageKernelError::SchemaVersionMismatch { expected, found },
            ))) if expected == SPIRIT_STORE_CURRENT_SCHEMA_VERSION
                && (found == SPIRIT_STORE_V1_SCHEMA_VERSION
                    || found == SPIRIT_STORE_V2_SCHEMA_VERSION
                    || found == SPIRIT_STORE_V3_SCHEMA_VERSION
                    || found == SPIRIT_STORE_V4_SCHEMA_VERSION
                    || found == SPIRIT_STORE_V5_SCHEMA_VERSION
                    || found == SPIRIT_STORE_V6_SCHEMA_VERSION) =>
            {
                self.upgrade_previous_store(database_path, found)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn upgrade_previous_store(
        &self,
        database_path: PathBuf,
        previous_schema_version: SchemaVersion,
    ) -> Result<SpiritStoreUpgradeOutput, ProductionMigrationError> {
        let records = match previous_schema_version {
            SPIRIT_STORE_V1_SCHEMA_VERSION => {
                SpiritStorePreviousRecords::from_v1(SpiritStoreV1Database::open(&database_path)?)
            }
            SPIRIT_STORE_V2_SCHEMA_VERSION | SPIRIT_STORE_V3_SCHEMA_VERSION => {
                SpiritStorePreviousRecords::from_v2(SpiritStoreV2Database::open(
                    &database_path,
                    previous_schema_version,
                )?)
            }
            SPIRIT_STORE_V4_SCHEMA_VERSION => {
                SpiritStorePreviousRecords::from_v4(SpiritStoreV4Database::open(&database_path)?)
            }
            SPIRIT_STORE_V5_SCHEMA_VERSION => {
                SpiritStorePreviousRecords::from_v5(SpiritStoreV5Database::open(&database_path)?)
            }
            SPIRIT_STORE_V6_SCHEMA_VERSION => {
                SpiritStorePreviousRecords::from_v6(SpiritStoreV6Database::open(&database_path)?)
            }
            _ => unreachable!("upgrade is only called for known previous schema versions"),
        }?;
        let temporary_path = Self::temporary_path(&database_path);
        if temporary_path.exists() {
            fs::remove_file(&temporary_path)?;
        }
        let target_store = Store::open(&temporary_path)?;
        let record_count = records.len();
        for record in records.into_records() {
            target_store.import_record(record.record_identifier, record.entry)?;
        }
        let backup_path = Self::backup_path(&database_path);
        fs::rename(&database_path, backup_path)?;
        fs::rename(temporary_path, database_path)?;
        Ok(SpiritStoreUpgradeOutput::upgraded(
            SpiritStoreUpgradeCompleted {
                record_count: record_count as u64,
            },
        ))
    }

    fn temporary_path(database_path: &Path) -> PathBuf {
        database_path.with_extension(format!("schema-7-migrating-{}.sema", std::process::id()))
    }

    fn backup_path(database_path: &Path) -> PathBuf {
        for suffix in 0.. {
            let candidate =
                database_path.with_extension(format!("schema-old-backup-{suffix}.sema"));
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

impl SpiritStoreV2Database {
    fn open(path: &Path, schema_version: SchemaVersion) -> Result<Self, ProductionMigrationError> {
        let mut database = SemaDatabase::open(EngineOpen::new(path, schema_version))?;
        let records = database.register_table(TableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV2Record>, ProductionMigrationError> {
        Ok(self
            .database
            .match_records(QueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV4Database {
    fn open(path: &Path) -> Result<Self, ProductionMigrationError> {
        let mut database =
            SemaDatabase::open(EngineOpen::new(path, SPIRIT_STORE_V4_SCHEMA_VERSION))?;
        let records = database.register_table(TableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV4Record>, ProductionMigrationError> {
        Ok(self
            .database
            .match_records(QueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV5Database {
    fn open(path: &Path) -> Result<Self, ProductionMigrationError> {
        let mut database =
            SemaDatabase::open(EngineOpen::new(path, SPIRIT_STORE_V5_SCHEMA_VERSION))?;
        let records = database.register_table(TableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV5Record>, ProductionMigrationError> {
        Ok(self
            .database
            .match_records(QueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV6Database {
    fn open(path: &Path) -> Result<Self, ProductionMigrationError> {
        let mut database =
            SemaDatabase::open(EngineOpen::new(path, SPIRIT_STORE_V6_SCHEMA_VERSION))?;
        let records = database.register_table(TableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV6Record>, ProductionMigrationError> {
        Ok(self
            .database
            .match_records(QueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

struct SpiritStorePreviousRecords {
    records: Vec<SpiritStorePreviousRecord>,
}

struct SpiritStorePreviousRecord {
    record_identifier: String,
    entry: Entry,
}

impl SpiritStorePreviousRecords {
    fn from_v1(database: SpiritStoreV1Database) -> Result<Self, ProductionMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v1)
                .collect(),
        })
    }

    fn from_v2(database: SpiritStoreV2Database) -> Result<Self, ProductionMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v2)
                .collect(),
        })
    }

    fn from_v4(database: SpiritStoreV4Database) -> Result<Self, ProductionMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v4)
                .collect(),
        })
    }

    fn from_v5(database: SpiritStoreV5Database) -> Result<Self, ProductionMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v5)
                .collect(),
        })
    }

    fn from_v6(database: SpiritStoreV6Database) -> Result<Self, ProductionMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v6)
                .collect(),
        })
    }

    fn into_records(self) -> Vec<SpiritStorePreviousRecord> {
        self.records
    }

    fn len(&self) -> usize {
        self.records.len()
    }
}

impl SpiritStorePreviousRecord {
    fn from_v1(record: SpiritStoreV1Record) -> Self {
        Self {
            record_identifier: record.record_identifier,
            entry: record.entry.into_new_entry(),
        }
    }

    fn from_v2(record: SpiritStoreV2Record) -> Self {
        Self {
            record_identifier: record.record_identifier,
            entry: record.entry.into_new_entry(),
        }
    }

    fn from_v4(record: SpiritStoreV4Record) -> Self {
        Self {
            record_identifier: record.record_identifier,
            entry: record.entry.into_new_entry(),
        }
    }

    fn from_v5(record: SpiritStoreV5Record) -> Self {
        Self {
            record_identifier: record.record_identifier,
            entry: record.entry.into_new_entry(),
        }
    }

    fn from_v6(record: SpiritStoreV6Record) -> Self {
        Self {
            record_identifier: record.record_identifier,
            entry: record.entry.into_new_entry(),
        }
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

impl EngineRecord for SpiritStoreV1Record {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.record_identifier.clone())
    }
}

impl EngineRecord for SpiritStoreV2Record {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.record_identifier.clone())
    }
}

impl EngineRecord for SpiritStoreV4Record {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.record_identifier.clone())
    }
}

impl EngineRecord for SpiritStoreV5Record {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.record_identifier.clone())
    }
}

impl EngineRecord for SpiritStoreV6Record {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.record_identifier.clone())
    }
}

impl SpiritStoreV1Entry {
    fn into_new_entry(self) -> Entry {
        Entry {
            domains: self.categories.into_domains(),
            kind: self.kind,
            description: self.description,
            certainty: Certainty::new(self.magnitude),
            importance: Importance::new(Magnitude::Minimum),
            privacy: self.privacy,
            referents: Referents::new(Vec::new()),
        }
    }
}

impl SpiritStoreV2Entry {
    fn into_new_entry(self) -> Entry {
        Entry {
            domains: self.categories.into_domains(),
            kind: self.kind,
            description: self.description,
            certainty: self.certainty,
            importance: self.importance,
            privacy: self.privacy,
            referents: Referents::new(Vec::new()),
        }
    }
}

impl SpiritStoreV4Entry {
    fn into_new_entry(self) -> Entry {
        let _legacy_weight = self.weight;
        Entry {
            domains: self.categories.into_domains(),
            kind: self.kind,
            description: self.description,
            certainty: self.certainty,
            importance: self.importance,
            privacy: self.privacy,
            referents: Referents::new(Vec::new()),
        }
    }
}

impl SpiritStoreV5Entry {
    fn into_new_entry(self) -> Entry {
        let _legacy_weight = self.weight;
        Entry {
            domains: self.categories.into_domains(),
            kind: self.kind,
            description: self.description,
            certainty: self.certainty,
            importance: self.importance,
            privacy: self.privacy,
            referents: Referents::new(Vec::new()),
        }
    }
}

impl SpiritStoreV6Entry {
    fn into_new_entry(self) -> Entry {
        Entry {
            domains: self.categories.into_domains(),
            kind: self.kind,
            description: self.description,
            certainty: self.certainty,
            importance: self.importance,
            privacy: self.privacy,
            referents: Referents::new(Vec::new()),
        }
    }
}

impl ProductionStampedEntry {
    fn into_new_entry(self) -> Entry {
        Entry {
            domains: Domains::from_strings(
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
            importance: Importance::new(Magnitude::Minimum),
            privacy: Privacy::new(Self::magnitude_from(self.entry.privacy)),
            referents: Referents::new(Vec::new()),
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

impl LegacyTextCategories {
    fn into_domains(self) -> Domains {
        Domains::from_strings(
            self.0
                .into_iter()
                .map(|category| category.into_label())
                .collect(),
        )
    }
}

impl LegacyTextCategory {
    fn into_label(self) -> String {
        self.0
    }
}

impl LegacyCategories {
    fn into_domains(self) -> Domains {
        Domains::from_strings(
            self.0
                .into_iter()
                .map(|category| category.label().to_owned())
                .collect(),
        )
    }
}

impl LegacyCategory {
    fn label(self) -> &'static str {
        match self {
            Self::Being => "being",
            Self::Knowing => "knowing",
            Self::Meaning => "meaning",
            Self::Making => "making",
            Self::Relating => "relating",
            Self::Governing => "governing",
            Self::Caring => "caring",
            Self::Sustaining => "sustaining",
            Self::Dwelling => "dwelling",
            Self::Moving => "moving",
            Self::Valuing => "valuing",
            Self::Expressing => "expressing",
        }
    }
}

#[cfg(test)]
mod tests {
    use sema_engine::{Assertion, Engine as SemaDatabase, EngineOpen, TableDescriptor};

    use super::{
        PRODUCTION_SCHEMA_VERSION, ProductionMigration, ProductionMigrationRequest,
        ProductionStampedEntry, ProductionStoredRecord, RECORDS_TABLE,
    };
    use crate::{
        Store,
        schema::signal::{Craft, Domain, Magnitude},
    };

    #[test]
    fn migrates_production_version_five_records_into_current_store() {
        let temporary = tempfile::tempdir().expect("create migration sandbox");
        let source_database_path = temporary.path().join("production.sema");
        let target_database_path = temporary.path().join("current.sema");
        let production_identifier = signal_spirit::RecordIdentifier::new(42);
        let migrated_identifier = production_identifier.code();

        let mut production_database = SemaDatabase::open(EngineOpen::new(
            &source_database_path,
            PRODUCTION_SCHEMA_VERSION,
        ))
        .expect("open production database");
        let production_records = production_database
            .register_table(TableDescriptor::new(RECORDS_TABLE))
            .expect("register production records table");
        production_database
            .assert(Assertion::new(
                production_records,
                ProductionStoredRecord {
                    identifier: production_identifier,
                    entry: ProductionStampedEntry {
                        entry: signal_spirit::Entry {
                            topics: signal_spirit::Topics::new(vec![signal_spirit::Topic::new(
                                "schema",
                            )]),
                            kind: signal_spirit::Kind::Decision,
                            description: signal_spirit::Description::new(
                                "production record survives migration",
                            ),
                            certainty: signal_spirit::Magnitude::High,
                            privacy: signal_spirit::Magnitude::Zero,
                        },
                        date: signal_spirit::Date::new(2026, 6, 11),
                        time: signal_spirit::Time::new(12, 0, 0),
                    },
                },
            ))
            .expect("seed production record");
        drop(production_database);

        let completed = ProductionMigration::new(ProductionMigrationRequest::new(
            source_database_path.display().to_string(),
            target_database_path.display().to_string(),
        ))
        .run()
        .expect("run production migration");
        let target_store = Store::open(target_database_path).expect("open migrated store");
        let migrated_entry = target_store
            .entry_by_identifier(&migrated_identifier)
            .expect("query migrated entry")
            .expect("migrated entry exists");

        assert_eq!(completed.record_count(), 1);
        assert_eq!(
            migrated_entry.description.payload(),
            "production record survives migration"
        );
        assert_eq!(migrated_entry.kind, crate::schema::signal::Kind::Decision);
        assert_eq!(migrated_entry.certainty.payload(), &Magnitude::High);
        assert_eq!(migrated_entry.importance.payload(), &Magnitude::Minimum);
        assert_eq!(migrated_entry.privacy.payload(), &Magnitude::Zero);
        assert_eq!(migrated_entry.referents.payload(), &Vec::new());
        assert_eq!(
            migrated_entry.domains.payload(),
            &vec![Domain::Craft(Craft::Schema)]
        );
    }
}
