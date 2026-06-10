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
    Engine as SemaDatabase, EngineOpen, EngineRecord, QueryPlan, RecordKey, SchemaVersion,
    TableDescriptor, TableName,
};
use spirit::{
    Configuration, SignalTransport, Store,
    schema::signal::{
        Certainty, CertaintyChange, CertaintySelection, Description, Entry, Input, Kind, Magnitude,
        ObserverFilter, Output, Privacy, PrivacySelection, Query, RecordChange, RecordIdentifier,
        Statement, StatementText, TopicMatch, Topics,
    },
};
#[cfg(feature = "production-migration")]
use spirit::{ProductionMigrationOutput, ProductionMigrationRequest};
use tempfile::TempDir;

const PRODUCTION_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(5);
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

impl ProductionStoredRecord {
    fn record_identifier(&self) -> String {
        self.identifier.code()
    }

    fn into_new_entry(self) -> Entry {
        self.entry.into_new_entry()
    }

    fn first_topic(&self) -> Option<String> {
        self.entry
            .entry
            .topics
            .as_slice()
            .first()
            .map(|topic| topic.as_str().to_owned())
    }

    fn matches_topic(&self, topic: &str) -> bool {
        self.entry
            .entry
            .topics
            .as_slice()
            .iter()
            .any(|candidate| candidate.as_str() == topic)
    }
}

impl EngineRecord for ProductionStoredRecord {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.identifier.code())
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
            magnitude: Self::magnitude_from(self.entry.certainty),
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
        topic_match: TopicMatch::Any,
        kind: None,
        privacy_selection: PrivacySelection::Any,
        certainty_selection: CertaintySelection::Any,
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

    let observed_topic = production_records
        .iter()
        .find_map(ProductionStoredRecord::first_topic)
        .expect("production records should carry at least one topic");
    let expected_topic_count = production_records
        .iter()
        .filter(|record| record.matches_topic(&observed_topic))
        .count();
    let observed_query = Query {
        topic_match: TopicMatch::partial(Topics::from_strings(vec![observed_topic])),
        kind: None,
        privacy_selection: PrivacySelection::Any,
        certainty_selection: CertaintySelection::Any,
    };

    match sandbox.run_input(Input::count(observed_query.clone())) {
        Output::RecordsCounted(counted) => {
            assert_eq!(
                *counted.record_count.payload() as usize,
                expected_topic_count,
                "new spirit should count migrated production records by topic"
            );
        }
        other => panic!("expected RecordsCounted for migrated production records, got {other:?}"),
    }

    let stash_handle = match sandbox.run_input(Input::observe(observed_query)) {
        Output::RecordsStashed(stashed) => {
            assert_eq!(
                *stashed.record_count.payload() as usize,
                expected_topic_count,
                "new spirit should observe migrated production records by topic"
            );
            stashed.stash_handle.clone()
        }
        other => panic!("expected RecordsStashed for migrated production records, got {other:?}"),
    };

    match sandbox.run_input(Input::lookup_stash(stash_handle)) {
        Output::RecordsObserved(records) => {
            assert_eq!(
                records.record_set.len(),
                expected_topic_count,
                "stash lookup should return the migrated records matching the topic query"
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
        topics: Topics::from_strings(vec![String::from("sandbox-migration-check")]),
        kind: Kind::Decision,
        description: Description::new(String::from(
            "new spirit can write to migrated production database",
        )),
        magnitude: Magnitude::High,
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
            topics: Topics::from_strings(vec![String::from("sandbox-migration-check")]),
            kind: Kind::Correction,
            description: Description::new(String::from(
                "new spirit can mutate migrated production records",
            )),
            magnitude: Magnitude::VeryHigh,
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
        topic_match: TopicMatch::Any,
        kind: None,
        privacy_selection: PrivacySelection::Any,
        certainty_selection: CertaintySelection::Any,
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
