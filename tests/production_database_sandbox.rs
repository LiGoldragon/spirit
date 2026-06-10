use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "production-migration")]
use nota_next::{NotaEncode, NotaSource};
use sema_engine::{
    Assertion, Engine as SemaDatabase, EngineOpen, EngineRecord, QueryPlan, RecordKey,
    SchemaVersion, TableDescriptor, TableName,
};
use spirit::{
    Configuration, SignalTransport, Store,
    schema::signal::{
        Categories, CategoryMatch, Certainty, CertaintyChange, CertaintySelection, Description,
        Entry, ImportanceSelection, Input, Kind, Magnitude, ObserverFilter, Output, Privacy,
        PrivacySelection, Query, RecordChange, RecordIdentifier, Statement, StatementText, Weight,
    },
};
#[cfg(feature = "production-migration")]
use spirit::{
    ProductionMigrationOutput, ProductionMigrationRequest, SpiritStoreUpgradeOutput,
    SpiritStoreUpgradeRequest,
};
use tempfile::TempDir;

const PRODUCTION_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(5);
const SPIRIT_STORE_V1_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(1);
const SPIRIT_STORE_V2_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(2);
const SPIRIT_STORE_V3_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(3);
const SPIRIT_STORE_V4_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(4);
const RECORDS_TABLE: TableName = TableName::new("records");

struct ProductionSandbox {
    directory: TempDir,
    database_path: PathBuf,
    socket_path: PathBuf,
    source_database_path: PathBuf,
}

impl ProductionSandbox {
    fn from_environment() -> Self {
        let source = env::var("SPIRIT_PRODUCTION_DATABASE")
            .expect("SPIRIT_PRODUCTION_DATABASE must point to the production database copy source");
        let source_database_path = PathBuf::from(source);
        let directory = TempDir::new().expect("create sandbox");
        let database_path = directory.path().join("production-copy.sema");
        fs::copy(&source_database_path, &database_path).unwrap_or_else(|error| {
            panic!(
                "copy production database from {} into sandbox: {error}",
                source_database_path.display()
            )
        });
        Self {
            socket_path: directory.path().join("spirit.sock"),
            directory,
            database_path,
            source_database_path,
        }
    }

    fn empty_from_environment() -> Self {
        let source = env::var("SPIRIT_PRODUCTION_DATABASE")
            .expect("SPIRIT_PRODUCTION_DATABASE must point to the production database copy source");
        let source_database_path = PathBuf::from(source);
        let directory = TempDir::new().expect("create sandbox");
        let copied_source_database_path = directory.path().join("production-source-copy.sema");
        fs::copy(&source_database_path, &copied_source_database_path).unwrap_or_else(|error| {
            panic!(
                "copy production database from {} into sandbox: {error}",
                source_database_path.display()
            )
        });
        Self {
            database_path: directory.path().join("migrated-production.sema"),
            socket_path: directory.path().join("spirit.sock"),
            directory,
            source_database_path: copied_source_database_path,
        }
    }

    fn configuration_path(&self) -> PathBuf {
        self.directory.path().join("configuration.rkyv")
    }

    fn meta_socket_path(&self) -> PathBuf {
        self.directory.path().join("meta-spirit.sock")
    }

    fn write_configuration(&self) {
        Configuration::new(&self.socket_path, &self.database_path)
            .with_meta_socket_path(self.meta_socket_path())
            .write_binary_file(self.configuration_path())
            .expect("write sandbox configuration");
    }

    fn spawn_daemon(&self) -> DaemonProcess {
        self.write_configuration();
        let child = Command::new(env!("CARGO_BIN_EXE_spirit-daemon"))
            .arg(self.configuration_path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn new spirit daemon against sandbox database");
        DaemonProcess::new(child, &self.socket_path, &self.meta_socket_path())
    }

    fn daemon_startup_failure(&self) -> String {
        self.write_configuration();
        let output = Command::new(env!("CARGO_BIN_EXE_spirit-daemon"))
            .arg(self.configuration_path())
            .output()
            .expect("run new spirit daemon against sandbox database");
        assert!(
            !output.status.success(),
            "daemon unexpectedly opened copied production database"
        );
        String::from_utf8(output.stderr).expect("daemon stderr is UTF-8")
    }

    fn production_database(&self) -> ProductionDatabase {
        ProductionDatabase::open(&self.source_database_path)
    }

    fn run_input(&self, input: Input) -> Output {
        let mut transport =
            SignalTransport::connect(&self.socket_path).expect("connect to sandbox daemon");
        let (_route, output) = transport.exchange(&input).expect("exchange input");
        output
    }
}

struct DaemonProcess {
    child: Child,
}

impl DaemonProcess {
    fn new(child: Child, working_socket_path: &Path, meta_socket_path: &Path) -> Self {
        let mut process = Self { child };
        process.wait_for_socket_or_exit(working_socket_path);
        process.wait_for_socket_or_exit(meta_socket_path);
        process
    }

    fn wait_for_socket_or_exit(&mut self, path: &Path) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if path.exists() {
                return;
            }
            if let Some(status) = self.child.try_wait().expect("poll daemon") {
                let mut stderr = String::new();
                if let Some(mut pipe) = self.child.stderr.take() {
                    use std::io::Read;
                    let _ = pipe.read_to_string(&mut stderr);
                }
                panic!(
                    "daemon exited before socket {} appeared: {status}\nstderr: {stderr}",
                    path.display()
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("socket did not appear at {}", path.display());
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct ProductionDatabase {
    database: SemaDatabase,
    records: sema_engine::TableReference<ProductionStoredRecord>,
}

struct SpiritStoreV1Database {
    database: SemaDatabase,
    records: sema_engine::TableReference<SpiritStoreV1Record>,
}

struct SpiritStoreV2Database {
    database: SemaDatabase,
    records: sema_engine::TableReference<SpiritStoreV2Record>,
}

struct SpiritStoreV4Database {
    database: SemaDatabase,
    records: sema_engine::TableReference<SpiritStoreV4Record>,
}

impl ProductionDatabase {
    fn open(path: &Path) -> Self {
        let mut database = SemaDatabase::open(EngineOpen::new(path, PRODUCTION_SCHEMA_VERSION))
            .expect("open copied production database through production schema version");
        let records = database
            .register_table(TableDescriptor::new(RECORDS_TABLE))
            .expect("register production records table");
        Self { database, records }
    }

    fn records(&self) -> Vec<ProductionStoredRecord> {
        self.database
            .match_records(QueryPlan::all(self.records))
            .expect("read copied production records")
            .records()
            .to_vec()
    }
}

impl SpiritStoreV1Database {
    fn create(path: &Path) -> Self {
        let mut database =
            SemaDatabase::open(EngineOpen::new(path, SPIRIT_STORE_V1_SCHEMA_VERSION))
                .expect("create schema-v1 spirit database");
        let records = database
            .register_table(TableDescriptor::new(RECORDS_TABLE))
            .expect("register schema-v1 records table");
        Self { database, records }
    }

    fn assert_record(&self, record: SpiritStoreV1Record) {
        self.database
            .assert(Assertion::new(self.records, record))
            .expect("assert schema-v1 record");
    }
}

impl SpiritStoreV2Database {
    fn create(path: &Path) -> Self {
        Self::create_with_schema_version(path, SPIRIT_STORE_V2_SCHEMA_VERSION)
    }

    fn create_with_schema_version(path: &Path, schema_version: SchemaVersion) -> Self {
        let mut database = SemaDatabase::open(EngineOpen::new(path, schema_version))
            .expect("create schema-v2/v3 spirit database");
        let records = database
            .register_table(TableDescriptor::new(RECORDS_TABLE))
            .expect("register schema-v2/v3 records table");
        Self { database, records }
    }

    fn assert_record(&self, record: SpiritStoreV2Record) {
        self.database
            .assert(Assertion::new(self.records, record))
            .expect("assert schema-v2 record");
    }
}

impl SpiritStoreV4Database {
    fn create(path: &Path) -> Self {
        let mut database =
            SemaDatabase::open(EngineOpen::new(path, SPIRIT_STORE_V4_SCHEMA_VERSION))
                .expect("create schema-v4 spirit database");
        let records = database
            .register_table(TableDescriptor::new(RECORDS_TABLE))
            .expect("register schema-v4 records table");
        Self { database, records }
    }

    fn assert_record(&self, record: SpiritStoreV4Record) {
        self.database
            .assert(Assertion::new(self.records, record))
            .expect("assert schema-v4 record");
    }
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
    categories: LegacyCategories,
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
    categories: LegacyCategories,
    kind: Kind,
    description: Description,
    certainty: Certainty,
    importance: spirit::schema::signal::Importance,
    privacy: Privacy,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV4Record {
    record_identifier: String,
    entry: SpiritStoreV4Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV4Entry {
    categories: LegacyCategories,
    kind: Kind,
    description: Description,
    certainty: Certainty,
    importance: spirit::schema::signal::Importance,
    weight: Weight,
    privacy: Privacy,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct LegacyCategory(String);

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct LegacyCategories(Vec<LegacyCategory>);

impl ProductionStoredRecord {
    fn record_identifier(&self) -> String {
        self.identifier.code()
    }

    fn into_new_entry(self) -> Entry {
        self.entry.into_new_entry()
    }

    fn first_category(&self) -> Option<String> {
        self.entry
            .entry
            .topics
            .as_slice()
            .first()
            .map(|category| category.as_str().to_owned())
    }

    fn matches_category(&self, category: &str) -> bool {
        self.entry
            .entry
            .topics
            .as_slice()
            .iter()
            .any(|candidate| candidate.as_str() == category)
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

impl ProductionStampedEntry {
    fn into_new_entry(self) -> Entry {
        Entry {
            categories: Categories::from_strings(
                self.entry
                    .topics
                    .as_slice()
                    .iter()
                    .map(|category| category.as_str().to_owned())
                    .collect(),
            ),
            kind: Self::kind_from(self.entry.kind),
            description: Description::new(self.entry.description.as_str().to_owned()),
            certainty: Self::magnitude_from(self.entry.certainty).into(),
            importance: Magnitude::Minimum.into(),
            weight: 1_u64.into(),
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

impl LegacyCategories {
    fn from_strings(categories: Vec<String>) -> Self {
        Self(categories.into_iter().map(LegacyCategory).collect())
    }
}

#[test]
#[ignore = "requires SPIRIT_PRODUCTION_DATABASE and copies the live production store into a sandbox"]
fn copied_production_database_requires_explicit_migration() {
    let sandbox = ProductionSandbox::from_environment();
    let stderr = sandbox.daemon_startup_failure();
    assert!(
        stderr.contains("schema version mismatch"),
        "direct-open failure should explain the production/new schema mismatch; stderr: {stderr}"
    );
    assert!(
        stderr.contains("v5") && stderr.contains("v1"),
        "direct-open failure should name the production and new schema versions; stderr: {stderr}"
    );
}

#[test]
#[ignore = "requires SPIRIT_PRODUCTION_DATABASE and migrates the copied production records into a sandbox"]
fn production_records_migrate_into_new_spirit_and_remain_queryable() {
    let sandbox = ProductionSandbox::empty_from_environment();
    let production_records = sandbox.production_database().records();
    assert!(
        !production_records.is_empty(),
        "production database copy should contain records"
    );
    let migration_store = Store::open(&sandbox.database_path).expect("open migration target");
    let mut migrated_identifiers = Vec::new();
    for production_record in production_records.clone() {
        let production_identifier = production_record.record_identifier();
        let imported = migration_store
            .import_record(
                production_identifier.clone(),
                production_record.into_new_entry(),
            )
            .expect("import production record into new Spirit store");
        assert_eq!(
            imported, production_identifier,
            "migration must preserve the production record identifier"
        );
        migrated_identifiers.push(imported);
    }
    drop(migration_store);
    let _daemon = sandbox.spawn_daemon();

    let first_migrated_identifier = migrated_identifiers[0].clone();
    assert_eq!(
        migrated_identifiers.len(),
        production_records.len(),
        "every production record should be imported into the new Spirit store"
    );
    assert_eq!(
        migrated_identifiers,
        production_records
            .iter()
            .map(ProductionStoredRecord::record_identifier)
            .collect::<Vec<_>>(),
        "migration must preserve production short/base36 record identifiers"
    );

    for record_identifier in &migrated_identifiers {
        match sandbox.run_input(Input::lookup(RecordIdentifier::new(
            record_identifier.clone(),
        ))) {
            Output::RecordFound(found) => {
                assert_eq!(found.record_identifier.payload(), record_identifier);
                assert!(
                    !found.entry.description.trim().is_empty(),
                    "migrated production record should resolve by its original identifier"
                );
            }
            Output::Error(report) => {
                panic!("lookup failed on migrated production store: {report:?}")
            }
            other => panic!("expected RecordFound for migrated identifier, got {other:?}"),
        }
    }

    let all_records_query = Query {
        category_match: CategoryMatch::Any,
        kind: None,
        privacy_selection: PrivacySelection::Any,
        certainty_selection: CertaintySelection::Any,
        importance_selection: ImportanceSelection::default_observation_importance(),
        weight_selection: spirit::schema::signal::WeightSelection::default_observation_weight(),
    };

    match sandbox.run_input(Input::count(all_records_query.clone())) {
        Output::RecordsCounted(counted) => {
            assert_eq!(
                *counted.record_count.payload() as usize,
                production_records.len(),
                "new spirit should count every migrated production record"
            );
        }
        other => panic!("expected all-record RecordsCounted after migration, got {other:?}"),
    }

    let all_records_stash = match sandbox.run_input(Input::observe(all_records_query)) {
        Output::RecordsStashed(stashed) => {
            assert_eq!(
                *stashed.record_count.payload() as usize,
                production_records.len(),
                "new spirit should observe every migrated production record"
            );
            stashed.stash_handle.clone()
        }
        other => panic!("expected all-record RecordsStashed after migration, got {other:?}"),
    };
    match sandbox.run_input(Input::lookup_stash(all_records_stash)) {
        Output::RecordsObserved(records) => {
            assert_eq!(
                records.record_set.len(),
                production_records.len(),
                "stash lookup should return every migrated production record"
            );
        }
        other => panic!("expected all-record RecordsObserved after migration, got {other:?}"),
    }

    let observed_category = production_records
        .iter()
        .find_map(ProductionStoredRecord::first_category)
        .expect("production records should carry at least one category");
    let expected_category_count = production_records
        .iter()
        .filter(|record| record.matches_category(&observed_category))
        .count();
    let observed_query = Query {
        category_match: CategoryMatch::partial(Categories::from_strings(vec![observed_category])),
        kind: None,
        privacy_selection: PrivacySelection::Any,
        certainty_selection: CertaintySelection::Any,
        importance_selection: ImportanceSelection::default_observation_importance(),
        weight_selection: spirit::schema::signal::WeightSelection::default_observation_weight(),
    };

    match sandbox.run_input(Input::count(observed_query.clone())) {
        Output::RecordsCounted(counted) => {
            assert_eq!(
                *counted.record_count.payload() as usize,
                expected_category_count,
                "new spirit should count migrated production records by category"
            );
        }
        other => panic!("expected RecordsCounted for migrated production records, got {other:?}"),
    }

    let stash_handle = match sandbox.run_input(Input::observe(observed_query)) {
        Output::RecordsStashed(stashed) => {
            assert_eq!(
                *stashed.record_count.payload() as usize,
                expected_category_count,
                "new spirit should observe migrated production records by category"
            );
            stashed.stash_handle.clone()
        }
        other => panic!("expected RecordsStashed for migrated production records, got {other:?}"),
    };

    match sandbox.run_input(Input::lookup_stash(stash_handle)) {
        Output::RecordsObserved(records) => {
            assert_eq!(
                records.record_set.len(),
                expected_category_count,
                "stash lookup should return the migrated records matching the category query"
            );
        }
        other => panic!("expected RecordsObserved from stash lookup, got {other:?}"),
    }

    match sandbox.run_input(Input::lookup(RecordIdentifier::new(
        first_migrated_identifier,
    ))) {
        Output::RecordFound(found) => {
            assert!(
                !found.entry.description.trim().is_empty(),
                "first migrated production record should resolve by its original identifier"
            );
        }
        Output::Error(report) => panic!("lookup failed on migrated production store: {report:?}"),
        other => panic!("expected RecordFound for first migrated identifier, got {other:?}"),
    }

    let mutation = sandbox.run_input(Input::record(Entry {
        categories: Categories::from_strings(vec![String::from("sandbox-migration-check")]),
        kind: Kind::Decision,
        description: Description::new(String::from(
            "new spirit can write to migrated production database",
        )),
        certainty: Magnitude::High.into(),
        importance: Magnitude::Minimum.into(),
        weight: 1_u64.into(),
        privacy: Privacy::new(Magnitude::Zero),
    }));
    let record_identifier = match mutation {
        Output::RecordAccepted(receipt) => receipt.record_identifier.clone(),
        other => panic!("expected RecordAccepted for sandbox mutation, got {other:?}"),
    };
    match sandbox.run_input(Input::change_certainty(CertaintyChange {
        record_identifier: record_identifier.clone(),
        certainty: Certainty::new(Magnitude::Medium),
    })) {
        Output::CertaintyChanged(receipt) => {
            assert_eq!(receipt.record_identifier, record_identifier);
            assert_eq!(receipt.certainty, Magnitude::Medium);
        }
        other => panic!("expected CertaintyChanged for sandbox mutation, got {other:?}"),
    }
    match sandbox.run_input(Input::change_record(RecordChange {
        record_identifier: record_identifier.clone(),
        entry: Entry {
            categories: Categories::from_strings(vec![String::from("sandbox-migration-check")]),
            kind: Kind::Correction,
            description: Description::new(String::from(
                "new spirit can mutate migrated production records",
            )),
            certainty: Magnitude::VeryHigh.into(),
            importance: Magnitude::Minimum.into(),
            weight: 1_u64.into(),
            privacy: Privacy::new(Magnitude::Zero),
        },
    })) {
        Output::RecordChanged(receipt) => {
            assert_eq!(receipt.record_identifier, record_identifier);
        }
        other => panic!("expected RecordChanged for sandbox mutation, got {other:?}"),
    }
    match sandbox.run_input(Input::remove(record_identifier.clone())) {
        Output::RecordRemoved(receipt) => {
            assert_eq!(receipt.payload().record_identifier, record_identifier);
        }
        other => panic!("expected RecordRemoved for sandbox cleanup, got {other:?}"),
    }

    match sandbox.run_input(Input::state(Statement::new(StatementText::new(
        String::from("sandbox migration state classification"),
    )))) {
        Output::RecordAccepted(receipt) => {
            assert_ne!(receipt.record_identifier, record_identifier);
        }
        other => panic!("expected State to classify and record in sandbox, got {other:?}"),
    }

    let tap = sandbox.run_input(Input::tap(ObserverFilter::All));
    match tap {
        Output::ObservationTapped(subscription) => {
            assert!(
                !subscription.observed_operations.is_empty(),
                "operation tap should expose sandbox operation history"
            );
        }
        other => panic!("expected ObservationTapped, got {other:?}"),
    }
}

#[cfg(feature = "production-migration")]
#[test]
fn store_upgrade_binary_preserves_ids_and_adds_default_importance() {
    let directory = TempDir::new().expect("create upgrade sandbox");
    let database_path = directory.path().join("spirit.sema");
    let old_database = SpiritStoreV1Database::create(&database_path);
    old_database.assert_record(SpiritStoreV1Record {
        record_identifier: String::from("wxyz"),
        entry: SpiritStoreV1Entry {
            categories: LegacyCategories::from_strings(vec![String::from("upgrade")]),
            kind: Kind::Decision,
            description: Description::new(String::from("old store record")),
            magnitude: Magnitude::High,
            privacy: Privacy::new(Magnitude::Zero),
        },
    });
    drop(old_database);

    let request =
        SpiritStoreUpgradeRequest::new(database_path.to_string_lossy().into_owned()).to_nota();
    let output = Command::new(env!("CARGO_BIN_EXE_spirit-upgrade-store"))
        .arg(request)
        .output()
        .expect("run store upgrade binary");
    assert!(
        output.status.success(),
        "upgrade stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("upgrade stdout UTF-8");
    let decoded = NotaSource::new(stdout.trim())
        .parse::<SpiritStoreUpgradeOutput>()
        .expect("upgrade stdout is typed NOTA");
    let SpiritStoreUpgradeOutput::Upgraded(completed) = decoded else {
        panic!("expected an actual schema-v1 to schema-v5 upgrade, got {decoded:?}");
    };
    assert_eq!(completed.record_count(), 1);

    let store = Store::open(&database_path).expect("open upgraded schema-v5 store");
    let upgraded = store
        .entry_by_identifier("wxyz")
        .expect("read upgraded store")
        .expect("upgraded record keeps original identifier");
    assert_eq!(upgraded.description, "old store record");
    assert_eq!(upgraded.certainty, Magnitude::High);
    assert_eq!(upgraded.importance.payload(), &Magnitude::Minimum);
    assert_eq!(upgraded.weight.payload(), &1);
    assert_eq!(upgraded.privacy, Magnitude::Zero);
}

#[cfg(feature = "production-migration")]
#[test]
fn store_upgrade_binary_preserves_schema_v2_importance() {
    let directory = TempDir::new().expect("create upgrade sandbox");
    let database_path = directory.path().join("spirit.sema");
    let old_database = SpiritStoreV2Database::create(&database_path);
    old_database.assert_record(SpiritStoreV2Record {
        record_identifier: String::from("v2id"),
        entry: SpiritStoreV2Entry {
            categories: LegacyCategories::from_strings(vec![String::from("upgrade")]),
            kind: Kind::Principle,
            description: Description::new(String::from("schema v2 store record")),
            certainty: Certainty::new(Magnitude::Medium),
            importance: Magnitude::High.into(),
            privacy: Privacy::new(Magnitude::Zero),
        },
    });
    drop(old_database);

    let request =
        SpiritStoreUpgradeRequest::new(database_path.to_string_lossy().into_owned()).to_nota();
    let output = Command::new(env!("CARGO_BIN_EXE_spirit-upgrade-store"))
        .arg(request)
        .output()
        .expect("run store upgrade binary");
    assert!(
        output.status.success(),
        "upgrade stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("upgrade stdout UTF-8");
    let decoded = NotaSource::new(stdout.trim())
        .parse::<SpiritStoreUpgradeOutput>()
        .expect("upgrade stdout is typed NOTA");
    let SpiritStoreUpgradeOutput::Upgraded(completed) = decoded else {
        panic!("expected an actual schema-v2 to schema-v5 upgrade, got {decoded:?}");
    };
    assert_eq!(completed.record_count(), 1);

    let store = Store::open(&database_path).expect("open upgraded schema-v5 store");
    let upgraded = store
        .entry_by_identifier("v2id")
        .expect("read upgraded store")
        .expect("upgraded record keeps original identifier");
    assert_eq!(upgraded.description, "schema v2 store record");
    assert_eq!(upgraded.certainty, Magnitude::Medium);
    assert_eq!(upgraded.importance.payload(), &Magnitude::High);
    assert_eq!(upgraded.weight.payload(), &1);
    assert_eq!(upgraded.privacy, Magnitude::Zero);
}

#[cfg(feature = "production-migration")]
#[test]
fn store_upgrade_binary_preserves_schema_v3_importance_and_adds_weight() {
    let directory = TempDir::new().expect("create upgrade sandbox");
    let database_path = directory.path().join("spirit.sema");
    let old_database = SpiritStoreV2Database::create_with_schema_version(
        &database_path,
        SPIRIT_STORE_V3_SCHEMA_VERSION,
    );
    old_database.assert_record(SpiritStoreV2Record {
        record_identifier: String::from("v3id"),
        entry: SpiritStoreV2Entry {
            categories: LegacyCategories::from_strings(vec![String::from("upgrade")]),
            kind: Kind::Principle,
            description: Description::new(String::from("schema v3 store record")),
            certainty: Certainty::new(Magnitude::High),
            importance: Magnitude::VeryHigh.into(),
            privacy: Privacy::new(Magnitude::Zero),
        },
    });
    drop(old_database);

    let request =
        SpiritStoreUpgradeRequest::new(database_path.to_string_lossy().into_owned()).to_nota();
    let output = Command::new(env!("CARGO_BIN_EXE_spirit-upgrade-store"))
        .arg(request)
        .output()
        .expect("run store upgrade binary");
    assert!(
        output.status.success(),
        "upgrade stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("upgrade stdout UTF-8");
    let decoded = NotaSource::new(stdout.trim())
        .parse::<SpiritStoreUpgradeOutput>()
        .expect("upgrade stdout is typed NOTA");
    let SpiritStoreUpgradeOutput::Upgraded(completed) = decoded else {
        panic!("expected an actual schema-v3 to schema-v5 upgrade, got {decoded:?}");
    };
    assert_eq!(completed.record_count(), 1);

    let store = Store::open(&database_path).expect("open upgraded schema-v5 store");
    let upgraded = store
        .entry_by_identifier("v3id")
        .expect("read upgraded store")
        .expect("upgraded record keeps original identifier");
    assert_eq!(upgraded.description, "schema v3 store record");
    assert_eq!(upgraded.certainty, Magnitude::High);
    assert_eq!(upgraded.importance.payload(), &Magnitude::VeryHigh);
    assert_eq!(upgraded.weight.payload(), &1);
    assert_eq!(upgraded.privacy, Magnitude::Zero);
}

#[cfg(feature = "production-migration")]
#[test]
fn store_upgrade_binary_preserves_schema_v4_weight_and_adds_categories() {
    let directory = TempDir::new().expect("create upgrade sandbox");
    let database_path = directory.path().join("spirit.sema");
    let old_database = SpiritStoreV4Database::create(&database_path);
    old_database.assert_record(SpiritStoreV4Record {
        record_identifier: String::from("v4id"),
        entry: SpiritStoreV4Entry {
            categories: LegacyCategories::from_strings(vec![String::from("schema-rust-next")]),
            kind: Kind::Constraint,
            description: Description::new(String::from("schema v4 store record")),
            certainty: Certainty::new(Magnitude::VeryHigh),
            importance: Magnitude::High.into(),
            weight: Weight::new(7),
            privacy: Privacy::new(Magnitude::Zero),
        },
    });
    drop(old_database);

    let request =
        SpiritStoreUpgradeRequest::new(database_path.to_string_lossy().into_owned()).to_nota();
    let output = Command::new(env!("CARGO_BIN_EXE_spirit-upgrade-store"))
        .arg(request)
        .output()
        .expect("run store upgrade binary");
    assert!(
        output.status.success(),
        "upgrade stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("upgrade stdout UTF-8");
    let decoded = NotaSource::new(stdout.trim())
        .parse::<SpiritStoreUpgradeOutput>()
        .expect("upgrade stdout is typed NOTA");
    let SpiritStoreUpgradeOutput::Upgraded(completed) = decoded else {
        panic!("expected an actual schema-v4 to schema-v5 upgrade, got {decoded:?}");
    };
    assert_eq!(completed.record_count(), 1);

    let store = Store::open(&database_path).expect("open upgraded schema-v5 store");
    let upgraded = store
        .entry_by_identifier("v4id")
        .expect("read upgraded store")
        .expect("upgraded record keeps original identifier");
    assert_eq!(
        upgraded.categories,
        Categories::from_strings(vec![String::from("schema-rust-next")])
    );
    assert_eq!(upgraded.description, "schema v4 store record");
    assert_eq!(upgraded.certainty, Magnitude::VeryHigh);
    assert_eq!(upgraded.importance.payload(), &Magnitude::High);
    assert_eq!(upgraded.weight.payload(), &7);
    assert_eq!(upgraded.privacy, Magnitude::Zero);
}

#[cfg(feature = "production-migration")]
#[test]
#[ignore = "requires SPIRIT_PRODUCTION_DATABASE and runs the production migration binary in a sandbox"]
fn production_migration_binary_preserves_ids_and_writes_queryable_new_store() {
    let sandbox = ProductionSandbox::empty_from_environment();
    let production_records = sandbox.production_database().records();
    let request = ProductionMigrationRequest::new(
        sandbox.source_database_path.to_string_lossy().into_owned(),
        sandbox.database_path.to_string_lossy().into_owned(),
    )
    .to_nota();

    let output = Command::new(env!("CARGO_BIN_EXE_spirit-migrate-production"))
        .arg(request)
        .output()
        .expect("run production migration binary");
    assert!(
        output.status.success(),
        "migration stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("migration stdout UTF-8");
    let decoded = NotaSource::new(stdout.trim())
        .parse::<ProductionMigrationOutput>()
        .expect("migration stdout is real typed NOTA");
    let ProductionMigrationOutput::Completed(completed) = decoded;
    assert_eq!(completed.record_count() as usize, production_records.len());

    let _daemon = sandbox.spawn_daemon();
    let all_records_query = Query {
        category_match: CategoryMatch::Any,
        kind: None,
        privacy_selection: PrivacySelection::Any,
        certainty_selection: CertaintySelection::Any,
        importance_selection: ImportanceSelection::default_observation_importance(),
        weight_selection: spirit::schema::signal::WeightSelection::default_observation_weight(),
    };
    match sandbox.run_input(Input::count(all_records_query)) {
        Output::RecordsCounted(counted) => {
            assert_eq!(
                *counted.record_count.payload() as usize,
                production_records.len()
            );
        }
        other => panic!("expected count of migrated production records, got {other:?}"),
    }

    for record in production_records {
        let production_identifier = record.record_identifier();
        match sandbox.run_input(Input::lookup(RecordIdentifier::new(
            production_identifier.clone(),
        ))) {
            Output::RecordFound(found) => {
                assert_eq!(found.record_identifier.payload(), &production_identifier);
            }
            other => {
                panic!(
                    "expected production identifier lookup after binary migration, got {other:?}"
                )
            }
        }
    }
}
