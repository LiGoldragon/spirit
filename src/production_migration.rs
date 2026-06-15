//! Store migration as a logged fold.
//!
//! The previous store generations (schema versions 7 and 8) carry no
//! versioned operation log: spirit never opted into engine versioning before
//! schema version 9, and they predate the current engine's storage layout, so
//! the current engine refuses to open them at all. No pre-version-7 store
//! exists anywhere, so any schema version below 7 is rejected outright as an
//! unrecognized version rather than folded forward. This module therefore
//! reads them with `sema-engine-previous` — the engine generation that wrote
//! them — converts each row through the historical `From`-chain, and writes
//! every record into a fresh version-9 store THROUGH the current engine's
//! logged choke points. The migrated store's versioned commit log thereby
//! begins with a complete, replayable record of the migration — the first
//! complete history a spirit store has ever had — closed by a typed
//! [`Migration`] marker row that is itself logged and restorable.
//!
//! This bootstrap is not yet a previous-LOG fold: there is no previous log to
//! fold, so the fold input is the previous store's materialized rows. From
//! version 9 onward the versioned log is authoritative and the next schema
//! bump replays the previous store's log through the version chain instead.
//!
//! # Crash safety of the in-place swap
//!
//! The fold writes the fresh store beside the live one
//! (`<stem>.schema-10-migrating-<pid>.sema`), then swaps it into place with
//! single-rename exposure: the previous store first gets a second name — a
//! hard link at the backup path (`<stem>.schema-old-backup-<N>.sema`, first
//! free `N`) — and ONE atomic rename then moves the fresh store over the
//! live path. The live path is therefore never absent. The previous store's
//! bytes survive every crash:
//!
//! - crash before the backup link: the live path still holds the previous
//!   store, untouched (the previous engine opens it read-only); at most a
//!   stale `.schema-10-migrating-*` temporary remains. Recovery: re-run
//!   `spirit-migrate-store` — it removes the stale temporary and redoes the
//!   fold.
//! - crash between the backup link and the rename: the live path still holds
//!   the previous store; the backup path is a second name for the same
//!   bytes. Recovery: re-run the migration (it mints the next free backup
//!   suffix; the extra backup name may be deleted).
//! - crash after the rename: the migration is complete — the live path holds
//!   the migrated version-9 store, the backup path holds the previous store.
//!   A re-run reports `Current` and changes nothing.
//!
//! To roll back to the previous store, an operator stops the daemon and
//! copies the newest backup over the live path:
//! `cp <stem>.schema-old-backup-<N>.sema <stem>.sema`. The default archive
//! sibling (`<stem>.archive.sema`) is swapped with the same backup-link plus
//! single-rename pattern and recovers the same way.

use std::{
    fs,
    path::{Path, PathBuf},
};

use nota_next::{NotaDecode, NotaDecodeError, NotaEncode, NotaSource};
use sema_engine::{
    Engine as CurrentSemaDatabase, EngineOpen as CurrentEngineOpen,
    EngineRecord as CurrentEngineRecord, QueryPlan as CurrentQueryPlan,
    RecordKey as CurrentRecordKey, SchemaHash as CurrentSchemaHash,
    TableDescriptor as CurrentTableDescriptor, TableName as CurrentTableName,
    TableReference as CurrentTableReference,
};
use sema_engine_layout3::{
    Engine as Layout3SemaDatabase, EngineOpen as Layout3EngineOpen,
    EngineRecord as Layout3EngineRecord, QueryPlan as Layout3QueryPlan,
    RecordKey as Layout3RecordKey, TableDescriptor as Layout3TableDescriptor,
    TableName as Layout3TableName, TableReference as Layout3TableReference,
};
use sema_engine_previous::{
    Engine as PreviousSemaDatabase, EngineOpen as PreviousEngineOpen,
    EngineRecord as PreviousEngineRecord, QueryPlan as PreviousQueryPlan,
    RecordKey as PreviousRecordKey, SchemaVersion, StorageKernelError,
    TableDescriptor as PreviousTableDescriptor, TableName,
    TableReference as PreviousTableReference,
};
use thiserror::Error;

use crate::{
    Store, StoreError,
    schema::{
        sema::{
            MigratedRecordCount, MigratedReferentCount, Migration, SourceSchemaVersion,
            StoredRecord, StoredReferent,
        },
        signal::{
            Certainty, DataLeaf, Description, DistributedLeaf, Domain, Domains, EngineeringLeaf,
            Entry, HardwareLeaf, Importance, Information, IntelligenceLeaf, Kind,
            ObservabilityLeaf, OperationsLeaf, Privacy, ProgrammingLeaf, QualityLeaf,
            RecordIdentifier, Referent, Referents, SecurityLeaf, Software, SurfacesLeaf,
            SystemsLeaf, Technology,
        },
    },
    store::ArchiveDatabase,
};

const SPIRIT_STORE_V7_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(7);
const SPIRIT_STORE_V8_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(8);
const SPIRIT_STORE_V9_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(9);
const RECORDS_TABLE: TableName = TableName::new("records");
const REFERENTS_TABLE: TableName = TableName::new("referents");
const LAYOUT3_RECORDS_TABLE: Layout3TableName = Layout3TableName::new("records");
const LAYOUT3_REFERENTS_TABLE: Layout3TableName = Layout3TableName::new("referents");
const CURRENT_RECORDS_TABLE: CurrentTableName = CurrentTableName::new("records");
const CURRENT_REFERENTS_TABLE: CurrentTableName = CurrentTableName::new("referents");
const SPIRIT_STORE_V9_RECORDS_FAMILY: [u8; 32] = [
    128, 152, 120, 200, 198, 145, 69, 202, 209, 143, 69, 0, 194, 68, 236, 123, 66, 45, 71, 174, 61,
    91, 152, 128, 240, 76, 53, 227, 70, 162, 14, 185,
];
const SPIRIT_STORE_V9_REFERENTS_FAMILY: [u8; 32] = [
    148, 20, 243, 164, 221, 37, 189, 55, 141, 132, 255, 229, 160, 20, 221, 151, 9, 87, 17, 16, 190,
    49, 67, 192, 49, 14, 212, 75, 162, 60, 82, 157,
];

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct StoreMigrationRequest {
    database_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct StoreMigrationCompleted {
    record_count: u64,
    referent_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub enum StoreMigrationOutput {
    Current(StoreMigrationCompleted),
    Migrated(StoreMigrationCompleted),
}

#[derive(Debug, Error)]
pub enum StoreMigrationError {
    #[error("current spirit store: {0}")]
    CurrentSemaStore(#[from] sema_engine::Error),
    #[error("layout-3 spirit store: {0}")]
    Layout3Store(#[from] sema_engine_layout3::Error),
    #[error("previous spirit store: {0}")]
    PreviousStore(#[from] sema_engine_previous::Error),
    #[error("migrated spirit store: {0}")]
    Store(#[from] StoreError),
    #[error("store migration domain decode: {0}")]
    DomainDecode(#[from] NotaDecodeError),
    #[error("store migration io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unrecognized spirit store schema version: {found}")]
    UnknownSchemaVersion { found: SchemaVersion },
}

/// The in-place store migration: detect the source schema version, read the
/// previous store with the previous engine generation, and rebuild it as a
/// fresh versioned store whose log records the whole fold.
pub struct StoreMigration {
    request: StoreMigrationRequest,
}

/// The detected schema version of the store file at the requested path:
/// already current, or a previous generation the migration folds forward.
enum SourceStoreVersion {
    Current,
    VersionNineCurrent,
    VersionNineLayout3,
    Previous(SchemaVersion),
}

struct SpiritStoreV7Database {
    database: PreviousSemaDatabase,
    records: PreviousTableReference<SpiritStoreV7Record>,
    referents: PreviousTableReference<SpiritStoreV7Referent>,
}

/// A version-8 LIVE store: records plus the referent registry.
struct SpiritStoreV8LiveDatabase {
    database: PreviousSemaDatabase,
    records: PreviousTableReference<SpiritStoreV8Record>,
    referents: PreviousTableReference<SpiritStoreV8Referent>,
}

/// A version-8 ARCHIVE store: only the records table ever existed there.
/// A separate reader type keeps the archive open from registering — and
/// thereby writing a catalog row into — the referents table.
struct SpiritStoreV8ArchiveDatabase {
    database: PreviousSemaDatabase,
    records: PreviousTableReference<SpiritStoreV8Record>,
}

/// A version-9 store written by deployed spirit 0.12.1: materialized records
/// and referents are current, but the old layout-3 versioned log is not
/// decodable by the layout-5 engine. This reader folds the materialized state
/// into a fresh current store through the normal logged import choke points.
struct SpiritStoreV9Layout3LiveDatabase {
    database: Layout3SemaDatabase,
    records: Layout3TableReference<SpiritStoreV9Record>,
    referents: Layout3TableReference<StoredReferent>,
}

/// A version-9 store already rebuilt under the current layout-5 engine by the
/// 0.13 SEMA-VC tower, but still carrying the old fine-grained domain enum.
struct SpiritStoreV9CurrentLiveDatabase {
    database: CurrentSemaDatabase,
    records: CurrentTableReference<SpiritStoreV9Record>,
    referents: CurrentTableReference<StoredReferent>,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV7Record {
    record_identifier: String,
    entry: SpiritStoreV7Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV7Entry {
    domains: store_version_seven::Domains,
    kind: Kind,
    description: Description,
    certainty: Certainty,
    importance: Importance,
    privacy: Privacy,
    referents: Referents,
}

// The version-7 referent registry row shares the version-8 shape: a canonical
// referent and its aliases. Version 7 already registered referents, so a v7
// record whose entry carries a referent resolves against this table during the
// logged fold.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV7Referent {
    referent: Referent,
    aliases: Referents,
}

// The version-8 stored shapes: the last pre-versioning generation. The entry
// is already the current generated `Entry`; only the storage registration
// (hand-typed `String` identifier, no family identity, no versioned log)
// distinguishes v8 rows from the v9 store's schema-declared `StoredRecord`.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV8Record {
    record_identifier: String,
    entry: store_version_nine::Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV8Referent {
    referent: Referent,
    aliases: Referents,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV9Record {
    record_identifier: RecordIdentifier,
    entry: store_version_nine::Entry,
}

#[allow(clippy::enum_variant_names, dead_code)]
mod store_version_nine {
    use super::{
        Certainty, DataLeaf, Description, DistributedLeaf, Domain as CurrentDomain,
        Domains as CurrentDomains, EngineeringLeaf, Entry as CurrentEntry, HardwareLeaf,
        Importance, Information, IntelligenceLeaf, Kind, ObservabilityLeaf, OperationsLeaf,
        Privacy, ProgrammingLeaf, QualityLeaf, Referents, SecurityLeaf,
        Software as CurrentSoftware, SurfacesLeaf, SystemsLeaf, Technology as CurrentTechnology,
    };

    #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
    pub(super) struct Entry {
        pub(super) domains: Domains,
        pub(super) kind: Kind,
        pub(super) description: Description,
        pub(super) certainty: Certainty,
        pub(super) importance: Importance,
        pub(super) privacy: Privacy,
        pub(super) referents: Referents,
    }

    #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
    pub(super) struct Domains(Vec<Domain>);

    #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
    enum Domain {
        Health(super::store_version_seven::Health),
        Food(super::store_version_seven::Food),
        Home(super::store_version_seven::Home),
        Finance(super::store_version_seven::Finance),
        Work(super::store_version_seven::Work),
        Craft(super::store_version_seven::Craft),
        Knowledge(super::store_version_seven::Knowledge),
        Education(super::store_version_seven::Education),
        Language(super::store_version_seven::Language),
        Art(super::store_version_seven::Art),
        Kinship(super::store_version_seven::Kinship),
        Selfhood(super::store_version_seven::Selfhood),
        Spirituality(super::store_version_seven::Spirituality),
        Governance(super::store_version_seven::Governance),
        Law(super::store_version_seven::Law),
        Community(super::store_version_seven::Community),
        Nature(super::store_version_seven::Nature),
        Travel(super::store_version_seven::Travel),
        Commerce(super::store_version_seven::Commerce),
        Leisure(super::store_version_seven::Leisure),
        Appearance(super::store_version_seven::Appearance),
        Safety(super::store_version_seven::Safety),
        Information(super::store_version_seven::Information),
        Technology(Technology),
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Hardware {
        Energy,
        Power,
        Automation,
        Robotics,
        Networking,
        Materials,
        Machinery,
        Instrumentation,
        Aerospace,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Languages {
        ProgrammingLanguages,
        ProgrammingParadigms,
        TypeSystems,
        Compilation,
        Interpretation,
        Parsing,
        LexicalAnalysis,
        Grammars,
        CodeGeneration,
        Metaprogramming,
        Macros,
        DomainSpecificLanguages,
        RuntimeEnvironments,
        GarbageCollection,
        MemoryManagement,
        ForeignFunctionInterfaces,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Theory {
        Algorithms,
        DataStructures,
        ComputationalComplexity,
        AutomataTheory,
        FormalLanguages,
        GraphAlgorithms,
        TypeTheory,
        ProgramSemantics,
        FormalMethods,
        FormalVerification,
        ModelChecking,
        StaticAnalysis,
        NumericalComputing,
        Cryptanalysis,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Systems {
        OperatingSystems,
        SystemsProgramming,
        Concurrency,
        Parallelism,
        Asynchrony,
        Synchronization,
        Scheduling,
        FileSystems,
        Virtualization,
        Containerization,
        EmbeddedSystems,
        RealTimeSystems,
        Firmware,
        ResourceManagement,
        KernelDevelopment,
        DeviceDrivers,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Distributed {
        DistributedSystems,
        Networking,
        NetworkProtocols,
        ProtocolDesign,
        Consensus,
        Replication,
        MessageQueuing,
        EventDrivenArchitecture,
        ServiceMesh,
        LoadBalancing,
        RemoteProcedureCall,
        InterprocessCommunication,
        Routing,
        FaultTolerance,
        Sharding,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Data {
        DatabaseSystems,
        QueryProcessing,
        Indexing,
        Transactions,
        Caching,
        Storage,
        Persistence,
        Serialization,
        DataFormats,
        Compression,
        Encoding,
        DataModeling,
        DataPipelines,
        StreamProcessing,
        BatchProcessing,
        SchemaEvolution,
        DataMigration,
        DataValidation,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Intelligence {
        MachineLearning,
        DeepLearning,
        NeuralNetworks,
        NaturalLanguageProcessing,
        ComputerVision,
        ReinforcementLearning,
        ModelTraining,
        ModelInference,
        FeatureEngineering,
        PromptEngineering,
        RetrievalAugmentedGeneration,
        AgentSystems,
        InformationRetrieval,
        Search,
        Ranking,
        RecommendationSystems,
        KnowledgeRepresentation,
        AutomatedReasoning,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Security {
        Cryptography,
        Authentication,
        Authorization,
        AccessControl,
        AdmissionControl,
        SecretsManagement,
        ThreatModeling,
        VulnerabilityManagement,
        PenetrationTesting,
        ApplicationSecurity,
        NetworkSecurity,
        Sandboxing,
        Hardening,
        Privacy,
        IntrusionDetection,
        ReverseEngineering,
        InputSanitization,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Quality {
        Testing,
        UnitTesting,
        IntegrationTesting,
        EndToEndTesting,
        PropertyBasedTesting,
        Fuzzing,
        TestAutomation,
        Mocking,
        CodeCoverage,
        Debugging,
        Profiling,
        Benchmarking,
        PerformanceOptimization,
        LoadTesting,
        CodeReview,
        Refactoring,
        Linting,
        Formatting,
        TechnicalDebt,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Operations {
        ContinuousIntegration,
        ContinuousDelivery,
        BuildSystem,
        ReleaseEngineering,
        DependencyManagement,
        PackageManagement,
        ArtifactManagement,
        Deployment,
        Provisioning,
        InfrastructureAsCode,
        Orchestration,
        ConfigurationManagement,
        AutoScaling,
        CapacityPlanning,
        SiteReliability,
        IncidentResponse,
        DisasterRecovery,
        RateLimiting,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Observability {
        Logging,
        Monitoring,
        Alerting,
        Tracing,
        DistributedTracing,
        Metrics,
        Telemetry,
        ErrorHandling,
        AuditLogging,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Surfaces {
        WebDevelopment,
        FrontendDevelopment,
        BackendDevelopment,
        UserInterface,
        InteractionDesign,
        Rendering,
        ComputerGraphics,
        Animation,
        Layout,
        Styling,
        StateManagement,
        Accessibility,
        Usability,
        Internationalization,
        Localization,
        MobileDevelopment,
        GameDevelopment,
        Visualization,
        SyntaxHighlighting,
        CommandLineInterfaces,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    enum Engineering {
        SoftwareArchitecture,
        SoftwareDesign,
        DesignPatterns,
        DomainDrivenDesign,
        ApplicationProgrammingInterfaces,
        Microservices,
        Serverless,
        CloudComputing,
        EdgeComputing,
        Scalability,
        Reliability,
        Maintainability,
        Portability,
        Interoperability,
        Modularity,
        Abstraction,
        RequirementsEngineering,
        Documentation,
        VersionControl,
        SoftwareDevelopmentProcess,
        SoftwareMaintenance,
        SoftwareEngineeringManagement,
    }

    #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
    enum Software {
        Languages(Languages),
        Theory(Theory),
        Systems(Systems),
        Distributed(Distributed),
        Data(Data),
        Intelligence(Intelligence),
        Security(Security),
        Quality(Quality),
        Operations(Operations),
        Observability(Observability),
        Surfaces(Surfaces),
        Engineering(Engineering),
    }

    #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
    enum Technology {
        Hardware(Hardware),
        Software(Software),
    }

    pub(super) struct DomainRewrite {
        pub(super) domain: CurrentDomain,
        pub(super) keyword: Option<&'static str>,
    }

    impl Entry {
        pub(super) fn into_current(self) -> CurrentEntry {
            let (domains, description) = self.domains.into_current(self.description);
            CurrentEntry {
                domains,
                kind: self.kind,
                description,
                certainty: self.certainty,
                importance: self.importance,
                privacy: self.privacy,
                referents: self.referents,
            }
        }
    }

    #[cfg(test)]
    impl Entry {
        pub(super) fn documentation(
            description: &str,
            referents: Vec<super::Referent>,
            certainty: Certainty,
            importance: Importance,
            privacy: Privacy,
        ) -> Self {
            Self {
                domains: Domains(vec![Domain::Information(
                    super::store_version_seven::Information::Documentation,
                )]),
                kind: Kind::Decision,
                description: Description::new(description),
                certainty,
                importance,
                privacy,
                referents: Referents::new(referents),
            }
        }

        pub(super) fn data_query_processing(
            description: &str,
            certainty: Certainty,
            importance: Importance,
            privacy: Privacy,
        ) -> Self {
            Self::from_domain(
                Domain::Technology(Technology::Software(Software::Data(Data::QueryProcessing))),
                description,
                certainty,
                importance,
                privacy,
            )
        }

        fn from_domain(
            domain: Domain,
            description: &str,
            certainty: Certainty,
            importance: Importance,
            privacy: Privacy,
        ) -> Self {
            Self {
                domains: Domains(vec![domain]),
                kind: Kind::Decision,
                description: Description::new(description),
                certainty,
                importance,
                privacy,
                referents: Referents::new(Vec::new()),
            }
        }
    }

    impl Domains {
        fn into_current(self, description: Description) -> (CurrentDomains, Description) {
            let mut domains = Vec::new();
            let mut keywords = Vec::new();
            for domain in self.0 {
                let rewrite = domain.into_current();
                if !domains.iter().any(|existing| existing == &rewrite.domain) {
                    domains.push(rewrite.domain);
                }
                if let Some(keyword) = rewrite.keyword
                    && !keywords.iter().any(|existing| existing == &keyword)
                {
                    keywords.push(keyword);
                }
            }
            (
                CurrentDomains::new(domains),
                Description::new(DescriptionKeywordAppend::new(description, keywords).finish()),
            )
        }
    }

    struct DescriptionKeywordAppend {
        description: Description,
        keywords: Vec<&'static str>,
    }

    impl DescriptionKeywordAppend {
        fn new(description: Description, keywords: Vec<&'static str>) -> Self {
            Self {
                description,
                keywords,
            }
        }

        fn finish(self) -> String {
            let mut text = self.description.into_payload();
            for keyword in self.keywords {
                let marker = format!("*{keyword}*");
                if !text.to_lowercase().contains(&marker.to_lowercase()) {
                    if !text.ends_with(' ') && !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&marker);
                }
            }
            text
        }
    }

    impl Domain {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::Technology(value) => value.into_current(),
                Self::Information(super::store_version_seven::Information::Database) => {
                    DomainRewrite::same(CurrentDomain::Information(Information::Database))
                }
                Self::Information(value) => same_path("Information", value),
                Self::Health(value) => same_path("Health", value),
                Self::Food(value) => same_path("Food", value),
                Self::Home(value) => same_path("Home", value),
                Self::Finance(value) => same_path("Finance", value),
                Self::Work(value) => same_path("Work", value),
                Self::Craft(value) => same_path("Craft", value),
                Self::Knowledge(value) => same_path("Knowledge", value),
                Self::Education(value) => same_path("Education", value),
                Self::Language(value) => same_path("Language", value),
                Self::Art(value) => same_path("Art", value),
                Self::Kinship(value) => same_path("Kinship", value),
                Self::Selfhood(value) => same_path("Selfhood", value),
                Self::Spirituality(value) => same_path("Spirituality", value),
                Self::Governance(value) => same_path("Governance", value),
                Self::Law(value) => same_path("Law", value),
                Self::Community(value) => same_path("Community", value),
                Self::Nature(value) => same_path("Nature", value),
                Self::Travel(value) => same_path("Travel", value),
                Self::Commerce(value) => same_path("Commerce", value),
                Self::Leisure(value) => same_path("Leisure", value),
                Self::Appearance(value) => same_path("Appearance", value),
                Self::Safety(value) => same_path("Safety", value),
            }
        }
    }

    impl DomainRewrite {
        fn same(domain: CurrentDomain) -> Self {
            Self {
                domain,
                keyword: None,
            }
        }

        fn with_keyword(domain: CurrentDomain, keyword: &'static str) -> Self {
            Self {
                domain,
                keyword: Some(keyword),
            }
        }
    }

    impl Technology {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::Hardware(value) => value.into_current(),
                Self::Software(value) => value.into_current(),
            }
        }
    }

    impl Hardware {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::Networking => DomainRewrite::same(technology(CurrentTechnology::Hardware(
                    Some(HardwareLeaf::Networking),
                ))),
                Self::Energy => hardware_keyword("energy"),
                Self::Power => hardware_keyword("power"),
                Self::Automation => hardware_keyword("automation"),
                Self::Robotics => hardware_keyword("robotics"),
                Self::Materials => hardware_keyword("materials"),
                Self::Machinery => hardware_keyword("machinery"),
                Self::Instrumentation => hardware_keyword("instrumentation"),
                Self::Aerospace => hardware_keyword("aerospace"),
            }
        }
    }

    impl Software {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::Languages(value) => value.into_current(),
                Self::Theory(value) => value.into_current(),
                Self::Systems(value) => value.into_current(),
                Self::Distributed(value) => value.into_current(),
                Self::Data(value) => value.into_current(),
                Self::Intelligence(value) => value.into_current(),
                Self::Security(value) => value.into_current(),
                Self::Quality(value) => value.into_current(),
                Self::Operations(value) => value.into_current(),
                Self::Observability(value) => value.into_current(),
                Self::Surfaces(value) => value.into_current(),
                Self::Engineering(value) => value.into_current(),
            }
        }
    }

    impl Languages {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::ProgrammingLanguages => programming_terminal(),
                Self::TypeSystems => programming_leaf(ProgrammingLeaf::TypeSystems),
                Self::Compilation => programming_leaf(ProgrammingLeaf::Compilation),
                Self::Parsing => programming_leaf(ProgrammingLeaf::Parsing),
                Self::Grammars => programming_leaf(ProgrammingLeaf::Grammars),
                Self::CodeGeneration => programming_leaf(ProgrammingLeaf::CodeGeneration),
                Self::Metaprogramming => programming_leaf(ProgrammingLeaf::Metaprogramming),
                Self::Macros => programming_leaf(ProgrammingLeaf::Macros),
                Self::DomainSpecificLanguages => {
                    programming_leaf(ProgrammingLeaf::DomainSpecificLanguages)
                }
                Self::ProgrammingParadigms => programming_keyword("programming paradigms"),
                Self::Interpretation => programming_keyword("interpretation"),
                Self::LexicalAnalysis => programming_keyword("lexical analysis"),
                Self::RuntimeEnvironments => systems_keyword("runtime environments"),
                Self::GarbageCollection => systems_keyword("garbage collection"),
                Self::MemoryManagement => systems_keyword("memory management"),
                Self::ForeignFunctionInterfaces => {
                    programming_keyword("foreign function interfaces")
                }
            }
        }
    }

    impl Theory {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::FormalLanguages => programming_keyword("formal languages"),
                Self::TypeTheory => {
                    programming_leaf_with_keyword(ProgrammingLeaf::TypeSystems, "type theory")
                }
                Self::StaticAnalysis => quality_keyword("static analysis"),
                Self::FormalVerification | Self::ModelChecking => {
                    quality_keyword("formal verification")
                }
                Self::Cryptanalysis => security_keyword("cryptanalysis"),
                Self::Algorithms => theory_keyword("algorithms"),
                Self::DataStructures => theory_keyword("data structures"),
                Self::ComputationalComplexity => theory_keyword("computational complexity"),
                Self::AutomataTheory => theory_keyword("automata theory"),
                Self::GraphAlgorithms => theory_keyword("graph algorithms"),
                Self::ProgramSemantics => theory_keyword("program semantics"),
                Self::FormalMethods => theory_keyword("formal methods"),
                Self::NumericalComputing => theory_keyword("numerical computing"),
            }
        }
    }

    impl Systems {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::SystemsProgramming => systems_leaf(SystemsLeaf::SystemsProgramming),
                Self::Concurrency => systems_leaf(SystemsLeaf::Concurrency),
                Self::Parallelism => {
                    systems_leaf_with_keyword(SystemsLeaf::Concurrency, "parallelism")
                }
                Self::Asynchrony => {
                    systems_leaf_with_keyword(SystemsLeaf::Concurrency, "asynchrony")
                }
                Self::Synchronization => {
                    systems_leaf_with_keyword(SystemsLeaf::Concurrency, "synchronization")
                }
                Self::Scheduling => {
                    systems_leaf_with_keyword(SystemsLeaf::Concurrency, "scheduling")
                }
                Self::OperatingSystems => systems_keyword("operating systems"),
                Self::FileSystems => systems_keyword("file systems"),
                Self::Virtualization => systems_keyword("virtualization"),
                Self::Containerization => systems_keyword("containerization"),
                Self::EmbeddedSystems => systems_keyword("embedded systems"),
                Self::RealTimeSystems => systems_keyword("real-time systems"),
                Self::Firmware => systems_keyword("firmware"),
                Self::ResourceManagement => systems_keyword("resource management"),
                Self::KernelDevelopment => systems_keyword("kernel development"),
                Self::DeviceDrivers => systems_keyword("device drivers"),
            }
        }
    }

    impl Distributed {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::ProtocolDesign => distributed_leaf(DistributedLeaf::ProtocolDesign),
                Self::EventDrivenArchitecture => {
                    distributed_leaf(DistributedLeaf::EventDrivenArchitecture)
                }
                Self::InterprocessCommunication => {
                    distributed_leaf_with_keyword(DistributedLeaf::ProtocolDesign, "IPC")
                }
                Self::NetworkProtocols => distributed_leaf_with_keyword(
                    DistributedLeaf::ProtocolDesign,
                    "network protocols",
                ),
                Self::MessageQueuing => distributed_leaf_with_keyword(
                    DistributedLeaf::EventDrivenArchitecture,
                    "message queuing",
                ),
                Self::Networking => distributed_keyword("networking"),
                Self::DistributedSystems => distributed_keyword("distributed systems"),
                Self::Consensus => distributed_keyword("consensus"),
                Self::Replication => distributed_keyword("replication"),
                Self::ServiceMesh => distributed_keyword("service mesh"),
                Self::LoadBalancing => distributed_keyword("load balancing"),
                Self::RemoteProcedureCall => distributed_keyword("remote procedure call"),
                Self::Routing => distributed_keyword("routing"),
                Self::FaultTolerance => distributed_keyword("fault tolerance"),
                Self::Sharding => distributed_keyword("sharding"),
            }
        }
    }

    impl Data {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::Persistence => data_leaf(DataLeaf::Persistence),
                Self::Serialization => data_leaf(DataLeaf::Serialization),
                Self::DataFormats => data_leaf(DataLeaf::Formats),
                Self::DataModeling => data_leaf(DataLeaf::Modeling),
                Self::SchemaEvolution => data_leaf(DataLeaf::SchemaEvolution),
                Self::DataMigration => data_leaf(DataLeaf::Migration),
                Self::Encoding => data_leaf_with_keyword(DataLeaf::Serialization, "encoding"),
                Self::Compression => data_leaf_with_keyword(DataLeaf::Serialization, "compression"),
                Self::Storage => data_leaf_with_keyword(DataLeaf::Persistence, "storage"),
                Self::DatabaseSystems => data_keyword("database systems"),
                Self::QueryProcessing => data_keyword("query processing"),
                Self::Indexing => data_keyword("indexing"),
                Self::Transactions => data_keyword("transactions"),
                Self::Caching => data_keyword("caching"),
                Self::DataPipelines => data_keyword("data pipelines"),
                Self::StreamProcessing => data_keyword("stream processing"),
                Self::BatchProcessing => data_keyword("batch processing"),
                Self::DataValidation => data_keyword("validation"),
            }
        }
    }

    impl Intelligence {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::AgentSystems => intelligence_leaf(IntelligenceLeaf::AgentSystems),
                Self::MachineLearning => intelligence_keyword("machine learning"),
                Self::DeepLearning => intelligence_keyword("deep learning"),
                Self::NeuralNetworks => intelligence_keyword("neural networks"),
                Self::NaturalLanguageProcessing => {
                    intelligence_keyword("natural language processing")
                }
                Self::ComputerVision => intelligence_keyword("computer vision"),
                Self::ReinforcementLearning => intelligence_keyword("reinforcement learning"),
                Self::ModelTraining => intelligence_keyword("model training"),
                Self::ModelInference => intelligence_keyword("model inference"),
                Self::FeatureEngineering => intelligence_keyword("feature engineering"),
                Self::PromptEngineering => intelligence_keyword("prompt engineering"),
                Self::RetrievalAugmentedGeneration => {
                    intelligence_keyword("retrieval augmented generation")
                }
                Self::InformationRetrieval => intelligence_keyword("information retrieval"),
                Self::Search => intelligence_keyword("search"),
                Self::Ranking => intelligence_keyword("ranking"),
                Self::RecommendationSystems => intelligence_keyword("recommendation systems"),
                Self::KnowledgeRepresentation => intelligence_keyword("knowledge representation"),
                Self::AutomatedReasoning => intelligence_keyword("automated reasoning"),
            }
        }
    }

    impl Security {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::Cryptography => security_leaf(SecurityLeaf::Cryptography),
                Self::Authentication => security_leaf(SecurityLeaf::Authentication),
                Self::Authorization => security_leaf(SecurityLeaf::Authorization),
                Self::SecretsManagement => security_leaf(SecurityLeaf::SecretsManagement),
                Self::Privacy => security_leaf(SecurityLeaf::Privacy),
                Self::AccessControl => {
                    security_leaf_with_keyword(SecurityLeaf::Authorization, "access control")
                }
                Self::AdmissionControl => distributed_leaf_with_keyword(
                    DistributedLeaf::ProtocolDesign,
                    "admission control",
                ),
                Self::ThreatModeling => security_keyword("threat modeling"),
                Self::VulnerabilityManagement => security_keyword("vulnerability management"),
                Self::PenetrationTesting => security_keyword("penetration testing"),
                Self::ApplicationSecurity => security_keyword("application security"),
                Self::NetworkSecurity => security_keyword("network security"),
                Self::Sandboxing => security_keyword("sandboxing"),
                Self::Hardening => security_keyword("hardening"),
                Self::IntrusionDetection => security_keyword("intrusion detection"),
                Self::ReverseEngineering => security_keyword("reverse engineering"),
                Self::InputSanitization => security_keyword("input sanitization"),
            }
        }
    }

    impl Quality {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::Testing => quality_leaf(QualityLeaf::Testing),
                Self::UnitTesting => {
                    quality_leaf_with_keyword(QualityLeaf::Testing, "unit testing")
                }
                Self::IntegrationTesting => {
                    quality_leaf_with_keyword(QualityLeaf::Testing, "integration testing")
                }
                Self::EndToEndTesting => quality_leaf_with_keyword(QualityLeaf::Testing, "e2e"),
                Self::PropertyBasedTesting => {
                    quality_leaf_with_keyword(QualityLeaf::Testing, "property based testing")
                }
                Self::Fuzzing => quality_leaf_with_keyword(QualityLeaf::Testing, "fuzzing"),
                Self::TestAutomation => {
                    quality_leaf_with_keyword(QualityLeaf::Testing, "test automation")
                }
                Self::Mocking => quality_leaf_with_keyword(QualityLeaf::Testing, "mocking"),
                Self::CodeCoverage => quality_keyword("code coverage"),
                Self::Debugging => quality_keyword("debugging"),
                Self::Profiling => quality_keyword("profiling"),
                Self::Benchmarking => quality_keyword("benchmarking"),
                Self::PerformanceOptimization => quality_keyword("performance optimization"),
                Self::LoadTesting => quality_keyword("load testing"),
                Self::CodeReview => quality_keyword("code review"),
                Self::Refactoring => quality_keyword("refactoring"),
                Self::Linting => quality_keyword("linting"),
                Self::Formatting => quality_keyword("formatting"),
                Self::TechnicalDebt => quality_keyword("technical debt"),
            }
        }
    }

    impl Operations {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::BuildSystem => operations_leaf(OperationsLeaf::BuildSystem),
                Self::ReleaseEngineering => operations_leaf(OperationsLeaf::ReleaseEngineering),
                Self::DependencyManagement => operations_leaf(OperationsLeaf::DependencyManagement),
                Self::Deployment => operations_leaf(OperationsLeaf::Deployment),
                Self::ConfigurationManagement => {
                    operations_leaf(OperationsLeaf::ConfigurationManagement)
                }
                Self::ContinuousIntegration => {
                    operations_leaf_with_keyword(OperationsLeaf::BuildSystem, "CI")
                }
                Self::ContinuousDelivery => {
                    operations_leaf_with_keyword(OperationsLeaf::Deployment, "CD")
                }
                Self::PackageManagement => {
                    operations_leaf_with_keyword(OperationsLeaf::DependencyManagement, "packaging")
                }
                Self::ArtifactManagement => {
                    operations_leaf_with_keyword(OperationsLeaf::ReleaseEngineering, "artifacts")
                }
                Self::Provisioning => {
                    operations_leaf_with_keyword(OperationsLeaf::Deployment, "provisioning")
                }
                Self::InfrastructureAsCode => operations_leaf_with_keyword(
                    OperationsLeaf::Deployment,
                    "infrastructure as code",
                ),
                Self::Orchestration => {
                    operations_leaf_with_keyword(OperationsLeaf::Deployment, "orchestration")
                }
                Self::AutoScaling => operations_keyword("auto scaling"),
                Self::CapacityPlanning => operations_keyword("capacity planning"),
                Self::SiteReliability => operations_keyword("site reliability"),
                Self::IncidentResponse => operations_keyword("incident response"),
                Self::DisasterRecovery => operations_keyword("disaster recovery"),
                Self::RateLimiting => operations_keyword("rate limiting"),
            }
        }
    }

    impl Observability {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::Tracing => observability_leaf(ObservabilityLeaf::Tracing),
                Self::DistributedTracing => observability_leaf_with_keyword(
                    ObservabilityLeaf::Tracing,
                    "distributed tracing",
                ),
                Self::Logging => observability_keyword("logging"),
                Self::Monitoring => observability_keyword("monitoring"),
                Self::Alerting => observability_keyword("alerting"),
                Self::Metrics => observability_keyword("metrics"),
                Self::Telemetry => observability_keyword("telemetry"),
                Self::ErrorHandling => observability_keyword("error handling"),
                Self::AuditLogging => observability_keyword("audit logging"),
            }
        }
    }

    impl Surfaces {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::Visualization => surfaces_leaf(SurfacesLeaf::Visualization),
                Self::CommandLineInterfaces => surfaces_leaf(SurfacesLeaf::CommandLineInterfaces),
                Self::ComputerGraphics => {
                    surfaces_leaf_with_keyword(SurfacesLeaf::Visualization, "computer graphics")
                }
                Self::WebDevelopment => surfaces_keyword("web development"),
                Self::FrontendDevelopment => surfaces_keyword("frontend development"),
                Self::BackendDevelopment => surfaces_keyword("backend development"),
                Self::UserInterface => surfaces_keyword("user interface"),
                Self::InteractionDesign => surfaces_keyword("interaction design"),
                Self::Rendering => surfaces_keyword("rendering"),
                Self::Animation => surfaces_keyword("animation"),
                Self::Layout => surfaces_keyword("layout"),
                Self::Styling => surfaces_keyword("styling"),
                Self::StateManagement => surfaces_keyword("state management"),
                Self::Accessibility => surfaces_keyword("accessibility"),
                Self::Usability => surfaces_keyword("usability"),
                Self::Internationalization => surfaces_keyword("internationalization"),
                Self::Localization => surfaces_keyword("localization"),
                Self::MobileDevelopment => surfaces_keyword("mobile development"),
                Self::GameDevelopment => surfaces_keyword("game development"),
                Self::SyntaxHighlighting => surfaces_keyword("syntax highlighting"),
            }
        }
    }

    impl Engineering {
        fn into_current(self) -> DomainRewrite {
            match self {
                Self::SoftwareArchitecture => engineering_leaf(EngineeringLeaf::Architecture),
                Self::SoftwareDesign => engineering_leaf(EngineeringLeaf::Design),
                Self::ApplicationProgrammingInterfaces => {
                    engineering_leaf(EngineeringLeaf::ApplicationProgrammingInterfaces)
                }
                Self::Documentation => engineering_leaf(EngineeringLeaf::Documentation),
                Self::VersionControl => engineering_leaf(EngineeringLeaf::VersionControl),
                Self::SoftwareDevelopmentProcess => {
                    engineering_leaf(EngineeringLeaf::DevelopmentProcess)
                }
                Self::SoftwareEngineeringManagement => {
                    engineering_leaf(EngineeringLeaf::Management)
                }
                Self::Modularity => engineering_leaf(EngineeringLeaf::Modularity),
                Self::DesignPatterns => {
                    engineering_leaf_with_keyword(EngineeringLeaf::Design, "design pattern")
                }
                Self::SoftwareMaintenance => engineering_leaf_with_keyword(
                    EngineeringLeaf::DevelopmentProcess,
                    "maintenance",
                ),
                Self::DomainDrivenDesign => {
                    engineering_leaf_with_keyword(EngineeringLeaf::Design, "domain-driven design")
                }
                Self::RequirementsEngineering => engineering_leaf_with_keyword(
                    EngineeringLeaf::DevelopmentProcess,
                    "requirements",
                ),
                Self::Abstraction => {
                    engineering_leaf_with_keyword(EngineeringLeaf::Architecture, "abstraction")
                }
                Self::Microservices => engineering_keyword("microservices"),
                Self::Serverless => engineering_keyword("serverless"),
                Self::CloudComputing => engineering_keyword("cloud computing"),
                Self::EdgeComputing => engineering_keyword("edge computing"),
                Self::Scalability => engineering_keyword("scalability"),
                Self::Reliability => engineering_keyword("reliability"),
                Self::Maintainability => engineering_keyword("maintainability"),
                Self::Portability => engineering_keyword("portability"),
                Self::Interoperability => engineering_keyword("interoperability"),
            }
        }
    }

    fn technology(payload: CurrentTechnology) -> CurrentDomain {
        CurrentDomain::Technology(payload)
    }

    fn software(payload: CurrentSoftware) -> CurrentDomain {
        technology(CurrentTechnology::Software(payload))
    }

    fn hardware_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(technology(CurrentTechnology::Hardware(None)), keyword)
    }

    fn programming_terminal() -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Programming(None)))
    }

    fn programming_leaf(payload: ProgrammingLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Programming(Some(payload))))
    }

    fn programming_leaf_with_keyword(
        payload: ProgrammingLeaf,
        keyword: &'static str,
    ) -> DomainRewrite {
        DomainRewrite::with_keyword(
            software(CurrentSoftware::Programming(Some(payload))),
            keyword,
        )
    }

    fn programming_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Programming(None)), keyword)
    }

    fn theory_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Theory), keyword)
    }

    fn systems_leaf(payload: SystemsLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Systems(Some(payload))))
    }

    fn systems_leaf_with_keyword(payload: SystemsLeaf, keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Systems(Some(payload))), keyword)
    }

    fn systems_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Systems(None)), keyword)
    }

    fn distributed_leaf(payload: DistributedLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Distributed(Some(payload))))
    }

    fn distributed_leaf_with_keyword(
        payload: DistributedLeaf,
        keyword: &'static str,
    ) -> DomainRewrite {
        DomainRewrite::with_keyword(
            software(CurrentSoftware::Distributed(Some(payload))),
            keyword,
        )
    }

    fn distributed_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Distributed(None)), keyword)
    }

    fn data_leaf(payload: DataLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Data(Some(payload))))
    }

    fn data_leaf_with_keyword(payload: DataLeaf, keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Data(Some(payload))), keyword)
    }

    fn data_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Data(None)), keyword)
    }

    fn intelligence_leaf(payload: IntelligenceLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Intelligence(Some(payload))))
    }

    fn intelligence_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Intelligence(None)), keyword)
    }

    fn security_leaf(payload: SecurityLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Security(Some(payload))))
    }

    fn security_leaf_with_keyword(payload: SecurityLeaf, keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Security(Some(payload))), keyword)
    }

    fn security_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Security(None)), keyword)
    }

    fn quality_leaf(payload: QualityLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Quality(Some(payload))))
    }

    fn quality_leaf_with_keyword(payload: QualityLeaf, keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Quality(Some(payload))), keyword)
    }

    fn quality_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Quality(None)), keyword)
    }

    fn operations_leaf(payload: OperationsLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Operations(Some(payload))))
    }

    fn operations_leaf_with_keyword(
        payload: OperationsLeaf,
        keyword: &'static str,
    ) -> DomainRewrite {
        DomainRewrite::with_keyword(
            software(CurrentSoftware::Operations(Some(payload))),
            keyword,
        )
    }

    fn operations_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Operations(None)), keyword)
    }

    fn observability_leaf(payload: ObservabilityLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Observability(Some(payload))))
    }

    fn observability_leaf_with_keyword(
        payload: ObservabilityLeaf,
        keyword: &'static str,
    ) -> DomainRewrite {
        DomainRewrite::with_keyword(
            software(CurrentSoftware::Observability(Some(payload))),
            keyword,
        )
    }

    fn observability_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Observability(None)), keyword)
    }

    fn surfaces_leaf(payload: SurfacesLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Surfaces(Some(payload))))
    }

    fn surfaces_leaf_with_keyword(payload: SurfacesLeaf, keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Surfaces(Some(payload))), keyword)
    }

    fn surfaces_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Surfaces(None)), keyword)
    }

    fn engineering_leaf(payload: EngineeringLeaf) -> DomainRewrite {
        DomainRewrite::same(software(CurrentSoftware::Engineering(Some(payload))))
    }

    fn engineering_leaf_with_keyword(
        payload: EngineeringLeaf,
        keyword: &'static str,
    ) -> DomainRewrite {
        DomainRewrite::with_keyword(
            software(CurrentSoftware::Engineering(Some(payload))),
            keyword,
        )
    }

    fn engineering_keyword(keyword: &'static str) -> DomainRewrite {
        DomainRewrite::with_keyword(software(CurrentSoftware::Engineering(None)), keyword)
    }

    fn same_path(area: &str, leaf: impl std::fmt::Debug) -> DomainRewrite {
        let source = format!("({area} {leaf:?})");
        DomainRewrite::same(
            super::NotaSource::new(&source)
                .parse::<CurrentDomain>()
                .expect("non-Technology domain shapes are unchanged"),
        )
    }
}

#[allow(dead_code)]
mod store_version_seven {
    use std::fmt;

    use super::{Domain as CurrentDomain, Domains as CurrentDomains, NotaDecodeError, NotaSource};

    #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
    pub(super) struct Domains(pub(super) Vec<Domain>);

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Health {
        Body,
        Mind,
        Nutrition,
        Exercise,
        Sleep,
        Medicine,
        Disease,
        Medication,
        Therapy,
        Reproduction,
        Sexuality,
        Aging,
        Disability,
        Addiction,
        Dentistry,
        Senses,
        Pain,
        Prevention,
        FirstAid,
        Rehabilitation,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Food {
        Cooking,
        Diet,
        Recipe,
        Baking,
        Preservation,
        Fermentation,
        Beverage,
        Entertaining,
        Foraging,
        Fasting,
        Dining,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Home {
        Housing,
        Maintenance,
        Renovation,
        Furnishing,
        Cleaning,
        Tidying,
        Relocation,
        Realty,
        Property,
        Utilities,
        Locksmithing,
        Appliances,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Finance {
        Budgeting,
        Saving,
        Spending,
        Debt,
        Credit,
        Investing,
        Retirement,
        Tax,
        Insurance,
        Income,
        Banking,
        Charity,
        Planning,
        Accounting,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Work {
        Career,
        JobSearch,
        Workplace,
        Vocation,
        Leadership,
        Entrepreneurship,
        Employment,
        Compensation,
        Scheduling,
        Unemployment,
        Freelancing,
        Teamwork,
        Productivity,
        Project,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Craft {
        Programming,
        Architecture,
        Schema,
        Infrastructure,
        Versioning,
        Testing,
        Electronics,
        Construction,
        Carpentry,
        Metalworking,
        Sewing,
        Manufacturing,
        Repair,
        Engineering,
        Tooling,
        Handicraft,
        Invention,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Knowledge {
        Mathematics,
        Logic,
        Physics,
        Chemistry,
        Biology,
        Astronomy,
        Geology,
        Computing,
        Physiology,
        Statistics,
        Research,
        History,
        Linguistics,
        Philosophy,
        Economics,
        Cognition,
        Taxonomy,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Education {
        Studying,
        Teaching,
        Schooling,
        Skill,
        Reading,
        Memorization,
        Pedagogy,
        Mentoring,
        Autodidacticism,
        Credential,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Language {
        Writing,
        Rhetoric,
        Translation,
        Grammar,
        Conversation,
        Correspondence,
        Listening,
        Oratory,
        Editing,
        Terminology,
        Notation,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Art {
        Fiction,
        Poetry,
        Music,
        Painting,
        Photography,
        Film,
        Theater,
        Dance,
        Design,
        Sculpture,
        Creativity,
        Storytelling,
        Publishing,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Kinship {
        Friendship,
        Romance,
        Marriage,
        Family,
        Parenting,
        Relatives,
        Reconciliation,
        Boundaries,
        Intimacy,
        Rapport,
        Caregiving,
        Grief,
        Belonging,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Selfhood {
        Growth,
        Introspection,
        Discipline,
        Emotion,
        Virtue,
        Motivation,
        Confidence,
        Identity,
        Purpose,
        Decision,
        Temperament,
        Wellbeing,
        Composure,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Spirituality {
        Worship,
        Prayer,
        Meditation,
        Ritual,
        Faith,
        Theology,
        Contemplation,
        Pilgrimage,
        Scripture,
        Ethics,
        Mortality,
        Transcendence,
        Asceticism,
        Wisdom,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Governance {
        Politics,
        Government,
        Administration,
        Citizenship,
        Elections,
        Activism,
        Policy,
        Diplomacy,
        Movements,
        Organizing,
        Services,
        Naturalization,
        War,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Law {
        Rights,
        Contract,
        Title,
        Crime,
        Litigation,
        Compliance,
        Custody,
        Liability,
        Procedure,
        Justice,
        Policing,
        Arbitration,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Community {
        Neighborliness,
        Volunteering,
        Solidarity,
        Membership,
        Gatherings,
        Reputation,
        Service,
        Hospitality,
        Institutions,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Nature {
        Agriculture,
        Gardening,
        Horticulture,
        Husbandry,
        Pets,
        Forestry,
        Fishing,
        Hunting,
        Conservation,
        Weather,
        Wilderness,
        Sustainability,
        Resources,
        Stewardship,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Travel {
        Itinerary,
        Destination,
        Transportation,
        Driving,
        Navigation,
        Commuting,
        Logistics,
        Migration,
        Tourism,
        Transit,
        Cycling,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Commerce {
        Selling,
        Buying,
        Marketing,
        Retail,
        Sourcing,
        Trade,
        Support,
        Pricing,
        Negotiation,
        Assets,
        Market,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Leisure {
        Recreation,
        Sport,
        Games,
        Hobby,
        Entertainment,
        Collecting,
        Outdoors,
        Play,
        Relaxation,
        Celebration,
        Fandom,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Appearance {
        Clothing,
        Grooming,
        Style,
        Cosmetics,
        Etiquette,
        Comportment,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Safety {
        Protection,
        Preparedness,
        Risk,
        Cybersecurity,
        Privacy,
        Disaster,
        Military,
        Deterrence,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Information {
        Curation,
        RecordKeeping,
        Documentation,
        News,
        Broadcasting,
        Archives,
        Database,
        Retrieval,
        Classification,
    }

    #[derive(
        rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq,
    )]
    pub(super) enum Technology {
        Energy,
        Power,
        Automation,
        Robotics,
        Intelligence,
        Networking,
        Materials,
        Machinery,
        Instrumentation,
        Aerospace,
    }

    #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
    pub(super) enum Domain {
        Health(Health),
        Food(Food),
        Home(Home),
        Finance(Finance),
        Work(Work),
        Craft(Craft),
        Knowledge(Knowledge),
        Education(Education),
        Language(Language),
        Art(Art),
        Kinship(Kinship),
        Selfhood(Selfhood),
        Spirituality(Spirituality),
        Governance(Governance),
        Law(Law),
        Community(Community),
        Nature(Nature),
        Travel(Travel),
        Commerce(Commerce),
        Leisure(Leisure),
        Appearance(Appearance),
        Safety(Safety),
        Information(Information),
        Technology(Technology),
    }

    impl Domains {
        pub(super) fn into_current(self) -> Result<CurrentDomains, NotaDecodeError> {
            Ok(CurrentDomains::new(
                self.0
                    .into_iter()
                    .map(Domain::into_current)
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
    }

    impl Domain {
        fn into_current(self) -> Result<CurrentDomain, NotaDecodeError> {
            let source = self.current_nota();
            NotaSource::new(&source).parse::<CurrentDomain>()
        }

        fn current_nota(self) -> String {
            match self {
                Self::Health(value) => Self::same_path("Health", value),
                Self::Food(value) => Self::same_path("Food", value),
                Self::Home(value) => Self::same_path("Home", value),
                Self::Finance(value) => Self::same_path("Finance", value),
                Self::Work(value) => Self::same_path("Work", value),
                Self::Craft(value) => value.current_nota(),
                Self::Knowledge(value) => Self::same_path("Knowledge", value),
                Self::Education(value) => Self::same_path("Education", value),
                Self::Language(value) => Self::same_path("Language", value),
                Self::Art(value) => Self::same_path("Art", value),
                Self::Kinship(value) => Self::same_path("Kinship", value),
                Self::Selfhood(value) => Self::same_path("Selfhood", value),
                Self::Spirituality(value) => Self::same_path("Spirituality", value),
                Self::Governance(value) => Self::same_path("Governance", value),
                Self::Law(value) => Self::same_path("Law", value),
                Self::Community(value) => Self::same_path("Community", value),
                Self::Nature(value) => Self::same_path("Nature", value),
                Self::Travel(value) => Self::same_path("Travel", value),
                Self::Commerce(value) => Self::same_path("Commerce", value),
                Self::Leisure(value) => Self::same_path("Leisure", value),
                Self::Appearance(value) => Self::same_path("Appearance", value),
                Self::Safety(value) => Self::same_path("Safety", value),
                Self::Information(value) => Self::same_path("Information", value),
                Self::Technology(value) => value.current_nota(),
            }
        }

        fn same_path(area: &str, leaf: impl fmt::Debug) -> String {
            format!("({area} {leaf:?})")
        }
    }

    impl Craft {
        fn current_nota(self) -> String {
            match self {
                Self::Programming => String::from("(Technology (Software Programming))"),
                Self::Architecture => {
                    String::from("(Technology (Software (Engineering Architecture)))")
                }
                Self::Schema => String::from("(Technology (Software (Data SchemaEvolution)))"),
                Self::Infrastructure => {
                    String::from("(Technology (Software (Operations Deployment)))")
                }
                Self::Versioning => {
                    String::from("(Technology (Software (Engineering VersionControl)))")
                }
                Self::Testing => String::from("(Technology (Software (Quality Testing)))"),
                Self::Tooling => String::from("(Technology (Software (Operations BuildSystem)))"),
                Self::Electronics
                | Self::Construction
                | Self::Carpentry
                | Self::Metalworking
                | Self::Sewing
                | Self::Manufacturing
                | Self::Repair
                | Self::Engineering
                | Self::Handicraft
                | Self::Invention => format!("(Craft {self:?})"),
            }
        }
    }

    impl Technology {
        fn current_nota(self) -> String {
            match self {
                Self::Intelligence => {
                    String::from("(Technology (Software (Intelligence AgentSystems)))")
                }
                Self::Energy
                | Self::Power
                | Self::Automation
                | Self::Robotics
                | Self::Networking
                | Self::Materials
                | Self::Machinery
                | Self::Instrumentation
                | Self::Aerospace => format!("(Technology (Hardware {self:?}))"),
            }
        }
    }
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

    pub fn referent_count(&self) -> u64 {
        self.referent_count
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
                referent_count: 0,
            }));
        }
        match self.source_store_version(&database_path)? {
            SourceStoreVersion::Current => match Store::open(&database_path) {
                Ok(store) => Ok(StoreMigrationOutput::current(StoreMigrationCompleted {
                    record_count: store.len() as u64,
                    referent_count: 0,
                })),
                Err(StoreError::Database { .. }) => self.migrate_version_nine_store(database_path),
                Err(error) => Err(error.into()),
            },
            SourceStoreVersion::VersionNineCurrent => {
                self.migrate_version_nine_store(database_path)
            }
            SourceStoreVersion::VersionNineLayout3 => {
                self.migrate_version_nine_layout3_store(database_path)
            }
            SourceStoreVersion::Previous(previous_schema_version) => {
                self.migrate_previous_store(database_path, previous_schema_version)
            }
        }
    }

    /// Probe the source store's schema version with the previous engine
    /// generation. The current engine cannot probe a pre-versioning store:
    /// it rejects the storage layout before reaching the schema version. The
    /// previous engine's open is read-only against an existing store, so the
    /// probe never mutates the source.
    fn source_store_version(
        &self,
        database_path: &Path,
    ) -> Result<SourceStoreVersion, StoreMigrationError> {
        if Store::open(database_path).is_ok() {
            return Ok(SourceStoreVersion::Current);
        }
        if SpiritStoreV9CurrentLiveDatabase::open(database_path).is_ok() {
            return Ok(SourceStoreVersion::VersionNineCurrent);
        }
        if SpiritStoreV9Layout3LiveDatabase::open(database_path).is_ok() {
            return Ok(SourceStoreVersion::VersionNineLayout3);
        }
        match PreviousSemaDatabase::open(PreviousEngineOpen::new(
            database_path,
            SPIRIT_STORE_V8_SCHEMA_VERSION,
        )) {
            Ok(_) => Ok(SourceStoreVersion::Previous(SPIRIT_STORE_V8_SCHEMA_VERSION)),
            Err(sema_engine_previous::Error::Sema(StorageKernelError::SchemaVersionMismatch {
                found,
                ..
            })) => {
                if found == SPIRIT_STORE_V9_SCHEMA_VERSION {
                    Ok(SourceStoreVersion::VersionNineLayout3)
                } else if found == SPIRIT_STORE_V7_SCHEMA_VERSION {
                    Ok(SourceStoreVersion::Previous(found))
                } else {
                    // No pre-version-7 store exists anywhere (psyche decision),
                    // so any schema version below 7 is unrecognized rather than
                    // a migratable previous generation.
                    Err(StoreMigrationError::UnknownSchemaVersion { found })
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    /// The deployed 0.12.1 v9 store is already on the current schema version,
    /// but its layout-3 versioned log cannot be decoded by the layout-5
    /// engine. Fold the materialized current rows through the deployed
    /// layout-3 engine into a fresh layout-5 store, producing a new complete
    /// versioned log from the preserved records and referents.
    fn migrate_version_nine_store(
        &self,
        database_path: PathBuf,
    ) -> Result<StoreMigrationOutput, StoreMigrationError> {
        let source =
            SpiritPreviousStore::from_v9(SpiritStoreV9CurrentLiveDatabase::open(&database_path)?)?;
        self.fold_previous_rows(database_path, source, SPIRIT_STORE_V9_SCHEMA_VERSION)
    }

    fn migrate_version_nine_layout3_store(
        &self,
        database_path: PathBuf,
    ) -> Result<StoreMigrationOutput, StoreMigrationError> {
        let source = SpiritPreviousStore::from_v9_layout3(SpiritStoreV9Layout3LiveDatabase::open(
            &database_path,
        )?)?;
        self.fold_previous_rows(database_path, source, SPIRIT_STORE_V9_SCHEMA_VERSION)
    }

    /// The logged fold: read every previous row with the previous engine,
    /// convert through the historical `From`-chain, write each record and
    /// referent into a fresh current store through the ordinary logged
    /// choke points, close with the typed [`Migration`] marker, then swap
    /// the fresh store into place. Identifiers survive unchanged.
    fn migrate_previous_store(
        &self,
        database_path: PathBuf,
        previous_schema_version: SchemaVersion,
    ) -> Result<StoreMigrationOutput, StoreMigrationError> {
        let source = match previous_schema_version {
            SPIRIT_STORE_V7_SCHEMA_VERSION => {
                SpiritPreviousStore::from_v7(SpiritStoreV7Database::open(&database_path)?)?
            }
            SPIRIT_STORE_V8_SCHEMA_VERSION => {
                SpiritPreviousStore::from_v8(SpiritStoreV8LiveDatabase::open(&database_path)?)?
            }
            _ => unreachable!("migration is only called for known previous schema versions"),
        };
        self.fold_previous_rows(database_path, source, previous_schema_version)
    }

    fn fold_previous_rows(
        &self,
        database_path: PathBuf,
        source: SpiritPreviousStore,
        source_schema_version: SchemaVersion,
    ) -> Result<StoreMigrationOutput, StoreMigrationError> {
        // Sweep every stale `*.schema-10-migrating-*.sema` temporary, not just
        // this process's: a crash under a different PID leaves a temporary
        // whose suffix this run would otherwise never touch, and the module
        // documentation promises a re-run clears the stale temporary.
        Self::sweep_stale_temporaries(&database_path)?;
        let temporary_path = Self::temporary_path(&database_path);
        let target_store = Store::open(&temporary_path)?;
        let record_count = source.records.len() as u64;
        let referent_count = source.referents.len() as u64;
        // Referents land first: record import canonicalizes entry referents
        // against the registered referent table.
        for referent in source.referents {
            target_store.import_referent(referent)?;
        }
        for record in source.records {
            target_store.import_record(record.record_identifier, record.entry)?;
        }
        target_store.record_migration(Migration {
            source_schema_version: SourceSchemaVersion::new(u64::from(
                source_schema_version.value(),
            )),
            migrated_record_count: MigratedRecordCount::new(record_count),
            migrated_referent_count: MigratedReferentCount::new(referent_count),
        })?;
        drop(target_store);
        self.migrate_archive_sibling(&database_path, source_schema_version)?;
        // Swap with single-rename exposure: first give the previous store a
        // second name (the backup hard link), then atomically rename the
        // fresh store over the live path. The live path is never absent — a
        // crash before the rename leaves the previous store live (plus a
        // backup name for the same bytes); a crash after it leaves the
        // migrated store live with the previous store intact at the backup
        // path. See the module documentation for operator recovery.
        let backup_path = Self::backup_path(&database_path);
        fs::hard_link(&database_path, backup_path)?;
        fs::rename(temporary_path, database_path)?;
        Ok(StoreMigrationOutput::migrated(StoreMigrationCompleted {
            record_count,
            referent_count,
        }))
    }

    /// Rebuild the default archive sibling (`<stem>.archive.sema`) under the
    /// current engine when one exists, preserving every archive key. The
    /// archive feature shipped in the version-8 era, so earlier sources have
    /// no sibling; an owner-configured non-default archive path is not
    /// covered and must be migrated by a separate request against that path.
    fn migrate_archive_sibling(
        &self,
        database_path: &Path,
        previous_schema_version: SchemaVersion,
    ) -> Result<(), StoreMigrationError> {
        if previous_schema_version != SPIRIT_STORE_V8_SCHEMA_VERSION {
            return Ok(());
        }
        let archive_path = Self::archive_sibling_path(database_path);
        if !archive_path.exists() {
            return Ok(());
        }
        let archive_records = SpiritStoreV8ArchiveDatabase::open(&archive_path)?.records()?;
        Self::sweep_stale_temporaries(&archive_path)?;
        let temporary_path = Self::temporary_path(&archive_path);
        let mut fresh_archive = ArchiveDatabase::open(&temporary_path)?;
        for record in archive_records {
            fresh_archive.import_archived_record(StoredRecord {
                record_identifier: RecordIdentifier::new(record.record_identifier),
                entry: record.entry.into_current(),
            })?;
        }
        drop(fresh_archive);
        // Same single-rename-exposure swap as the live store: backup hard
        // link first, then one atomic rename over the archive path.
        let backup_path = Self::backup_path(&archive_path);
        fs::hard_link(&archive_path, backup_path)?;
        fs::rename(temporary_path, archive_path)?;
        Ok(())
    }

    fn archive_sibling_path(database_path: &Path) -> PathBuf {
        let stem = database_path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("spirit"));
        database_path.with_file_name(format!("{stem}.archive.sema"))
    }

    fn temporary_path(database_path: &Path) -> PathBuf {
        database_path.with_extension(format!("schema-10-migrating-{}.sema", std::process::id()))
    }

    /// The shared prefix of every migration temporary for `database_path`:
    /// `temporary_path` replaces the `.sema` extension with
    /// `schema-10-migrating-<pid>.sema`, so each temporary's file name begins
    /// `<file-stem>.schema-10-migrating-`.
    fn temporary_name_prefix(database_path: &Path) -> String {
        let stem = database_path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("spirit"));
        format!("{stem}.schema-10-migrating-")
    }

    /// Remove every stale migration temporary beside `database_path`, matched
    /// by glob over the directory rather than only this process's PID-suffixed
    /// path. A crash under a different PID would otherwise strand a temporary
    /// this run never names; the swap is then guarded by the surviving live
    /// store, so clearing the stale temporary on re-run is safe.
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

impl SpiritStoreV7Database {
    /// Open a version-7 store: records PLUS the referent registry. Version 7
    /// already registered referents, so a v7 record whose entry carries a
    /// referent must resolve against this table — opening records alone would
    /// abort the logged fold on an unregistered referent.
    fn open(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            path,
            SPIRIT_STORE_V7_SCHEMA_VERSION,
        ))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        let referents = database.register_table(PreviousTableDescriptor::new(REFERENTS_TABLE))?;
        Ok(Self {
            database,
            records,
            referents,
        })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV7Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }

    fn referents(&self) -> Result<Vec<SpiritStoreV7Referent>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.referents))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV8LiveDatabase {
    fn open(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            path,
            SPIRIT_STORE_V8_SCHEMA_VERSION,
        ))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        let referents = database.register_table(PreviousTableDescriptor::new(REFERENTS_TABLE))?;
        Ok(Self {
            database,
            records,
            referents,
        })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV8Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }

    fn referents(&self) -> Result<Vec<SpiritStoreV8Referent>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.referents))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV8ArchiveDatabase {
    fn open(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            path,
            SPIRIT_STORE_V8_SCHEMA_VERSION,
        ))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV8Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV9Layout3LiveDatabase {
    fn open(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = Layout3SemaDatabase::open(
            Layout3EngineOpen::new(path, SPIRIT_STORE_V9_SCHEMA_VERSION).with_versioning(
                sema_engine_layout3::VersioningPolicy::new(
                    sema_engine_layout3::VersionedStoreName::new("spirit:sema"),
                ),
            ),
        )?;
        let records = database.register_table(Layout3TableDescriptor::new(
            LAYOUT3_RECORDS_TABLE,
            sema_engine_layout3::FamilyName::new("RecordsFamily"),
            sema_engine_layout3::SchemaHash::new(SPIRIT_STORE_V9_RECORDS_FAMILY),
        ))?;
        let referents = database.register_table(Layout3TableDescriptor::new(
            LAYOUT3_REFERENTS_TABLE,
            sema_engine_layout3::FamilyName::new("ReferentsFamily"),
            sema_engine_layout3::SchemaHash::new(SPIRIT_STORE_V9_REFERENTS_FAMILY),
        ))?;
        Ok(Self {
            database,
            records,
            referents,
        })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV9Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(Layout3QueryPlan::all(self.records))?
            .records()
            .to_vec())
    }

    fn referents(&self) -> Result<Vec<StoredReferent>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(Layout3QueryPlan::all(self.referents))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV9CurrentLiveDatabase {
    fn open(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = CurrentSemaDatabase::open(
            CurrentEngineOpen::new(path, SPIRIT_STORE_V9_SCHEMA_VERSION).with_versioning(
                sema_engine::VersioningPolicy::new(sema_engine::VersionedStoreName::new(
                    "spirit:sema",
                )),
            ),
        )?;
        let records = database.register_table(CurrentTableDescriptor::new(
            CURRENT_RECORDS_TABLE,
            sema_engine::FamilyName::new("RecordsFamily"),
            CurrentSchemaHash::new(SPIRIT_STORE_V9_RECORDS_FAMILY),
        ))?;
        let referents = database.register_table(CurrentTableDescriptor::new(
            CURRENT_REFERENTS_TABLE,
            sema_engine::FamilyName::new("ReferentsFamily"),
            CurrentSchemaHash::new(SPIRIT_STORE_V9_REFERENTS_FAMILY),
        ))?;
        Ok(Self {
            database,
            records,
            referents,
        })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV9Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(CurrentQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }

    fn referents(&self) -> Result<Vec<StoredReferent>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(CurrentQueryPlan::all(self.referents))?
            .records()
            .to_vec())
    }
}

/// The previous store's rows after conversion through the historical
/// `From`-chain: current-shape records plus the registered referents, ready
/// for the logged fold into a fresh store.
struct SpiritPreviousStore {
    records: Vec<SpiritPreviousRecord>,
    referents: Vec<StoredReferent>,
}

struct SpiritPreviousRecord {
    record_identifier: String,
    entry: Entry,
}

impl SpiritPreviousStore {
    fn from_v7(database: SpiritStoreV7Database) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritPreviousRecord::from_v7)
                .collect::<Result<Vec<_>, _>>()?,
            referents: database
                .referents()?
                .into_iter()
                .map(StoredReferent::from)
                .collect(),
        })
    }

    fn from_v8(database: SpiritStoreV8LiveDatabase) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritPreviousRecord::from_v8)
                .collect(),
            referents: database
                .referents()?
                .into_iter()
                .map(StoredReferent::from)
                .collect(),
        })
    }

    fn from_v9(database: SpiritStoreV9CurrentLiveDatabase) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritPreviousRecord::from_v9)
                .collect(),
            referents: database.referents()?,
        })
    }

    fn from_v9_layout3(
        database: SpiritStoreV9Layout3LiveDatabase,
    ) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritPreviousRecord::from_v9)
                .collect(),
            referents: database.referents()?,
        })
    }
}

impl SpiritPreviousRecord {
    fn from_v7(record: SpiritStoreV7Record) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            record_identifier: record.record_identifier,
            entry: record.entry.into_new_entry()?,
        })
    }

    fn from_v8(record: SpiritStoreV8Record) -> Self {
        Self {
            record_identifier: record.record_identifier,
            entry: record.entry.into_current(),
        }
    }

    fn from_v9(record: SpiritStoreV9Record) -> Self {
        Self {
            record_identifier: record.record_identifier.payload().clone(),
            entry: record.entry.into_current(),
        }
    }
}

impl From<SpiritStoreV7Referent> for StoredReferent {
    fn from(referent: SpiritStoreV7Referent) -> Self {
        Self {
            referent: referent.referent,
            aliases: referent.aliases,
        }
    }
}

impl From<SpiritStoreV8Referent> for StoredReferent {
    fn from(referent: SpiritStoreV8Referent) -> Self {
        Self {
            referent: referent.referent,
            aliases: referent.aliases,
        }
    }
}

impl PreviousEngineRecord for SpiritStoreV7Record {
    fn record_key(&self) -> PreviousRecordKey {
        PreviousRecordKey::new(self.record_identifier.clone())
    }
}

impl PreviousEngineRecord for SpiritStoreV7Referent {
    fn record_key(&self) -> PreviousRecordKey {
        PreviousRecordKey::new(self.referent.payload().clone())
    }
}

impl PreviousEngineRecord for SpiritStoreV8Record {
    fn record_key(&self) -> PreviousRecordKey {
        PreviousRecordKey::new(self.record_identifier.clone())
    }
}

impl PreviousEngineRecord for SpiritStoreV8Referent {
    fn record_key(&self) -> PreviousRecordKey {
        PreviousRecordKey::new(self.referent.payload().clone())
    }
}

impl Layout3EngineRecord for StoredRecord {
    fn record_key(&self) -> Layout3RecordKey {
        Layout3RecordKey::new(self.record_identifier.payload().clone())
    }
}

impl Layout3EngineRecord for SpiritStoreV9Record {
    fn record_key(&self) -> Layout3RecordKey {
        Layout3RecordKey::new(self.record_identifier.payload().clone())
    }
}

impl Layout3EngineRecord for StoredReferent {
    fn record_key(&self) -> Layout3RecordKey {
        Layout3RecordKey::new(self.referent.payload().clone())
    }
}

impl CurrentEngineRecord for SpiritStoreV9Record {
    fn record_key(&self) -> CurrentRecordKey {
        CurrentRecordKey::new(self.record_identifier.payload().clone())
    }
}

impl SpiritStoreV7Entry {
    fn into_new_entry(self) -> Result<Entry, StoreMigrationError> {
        Ok(Entry {
            domains: self.domains.into_current()?,
            kind: self.kind,
            description: self.description,
            certainty: self.certainty,
            importance: self.importance,
            privacy: self.privacy,
            referents: self.referents,
        })
    }
}

#[cfg(test)]
mod tests {
    use sema_engine_previous::{
        Assertion as PreviousAssertion, Engine as PreviousSemaDatabase,
        EngineOpen as PreviousEngineOpen, TableDescriptor as PreviousTableDescriptor,
    };

    use super::{
        CURRENT_RECORDS_TABLE, CURRENT_REFERENTS_TABLE, CurrentEngineOpen, CurrentSemaDatabase,
        CurrentTableDescriptor, RECORDS_TABLE, REFERENTS_TABLE, SPIRIT_STORE_V7_SCHEMA_VERSION,
        SPIRIT_STORE_V8_SCHEMA_VERSION, SPIRIT_STORE_V9_RECORDS_FAMILY,
        SPIRIT_STORE_V9_REFERENTS_FAMILY, SPIRIT_STORE_V9_SCHEMA_VERSION, SpiritStoreV7Entry,
        SpiritStoreV7Record, SpiritStoreV7Referent, SpiritStoreV8Record, SpiritStoreV8Referent,
        SpiritStoreV9Record, StoreMigration, StoreMigrationOutput, StoreMigrationRequest,
        store_version_nine, store_version_seven,
    };
    use crate::{
        Store,
        schema::{
            sema::RecordFamily,
            signal::{
                Certainty, DataLeaf, Description, Domain, Domains, Entry, Importance, Information,
                Kind, Magnitude, OperationsLeaf, Privacy, RecordIdentifier, Referent, Referents,
                Software, Technology,
            },
        },
    };
    use sema_engine::{
        Assertion as CurrentAssertion, FamilyName as CurrentFamilyName,
        SchemaHash as CurrentSchemaHash, VersionedStoreName as CurrentVersionedStoreName,
        VersioningPolicy as CurrentVersioningPolicy,
    };

    fn version_eight_entry(description: &str, referents: Vec<Referent>) -> Entry {
        Entry {
            domains: Domains::new(vec![Domain::Information(Information::Documentation)]),
            kind: Kind::Decision,
            description: Description::new(description),
            certainty: Certainty::new(Magnitude::High),
            importance: Importance::new(Magnitude::Medium),
            privacy: Privacy::new(Magnitude::Zero),
            referents: Referents::new(referents),
        }
    }

    fn version_eight_previous_entry(
        description: &str,
        referents: Vec<Referent>,
    ) -> store_version_nine::Entry {
        store_version_nine::Entry::documentation(
            description,
            referents,
            Certainty::new(Magnitude::High),
            Importance::new(Magnitude::Medium),
            Privacy::new(Magnitude::Zero),
        )
    }

    /// Seed a version-8 store — records, a registered referent, and an
    /// archive sibling — through the PREVIOUS engine generation, exactly as
    /// deployed spirit main wrote them.
    fn seed_version_eight_store(live_path: &std::path::Path, archive_path: &std::path::Path) {
        let mut live = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            live_path,
            SPIRIT_STORE_V8_SCHEMA_VERSION,
        ))
        .expect("open version eight live store");
        let records = live
            .register_table(PreviousTableDescriptor::new(RECORDS_TABLE))
            .expect("register version eight records table");
        let referents = live
            .register_table(PreviousTableDescriptor::new(REFERENTS_TABLE))
            .expect("register version eight referents table");
        live.assert(PreviousAssertion::new(
            referents,
            SpiritStoreV8Referent {
                referent: Referent::new("sema-engine"),
                aliases: Referents::new(vec![Referent::new("sema engine")]),
            },
        ))
        .expect("seed version eight referent");
        live.assert(PreviousAssertion::new(
            records,
            SpiritStoreV8Record {
                record_identifier: String::from("hj63"),
                entry: version_eight_previous_entry(
                    "version eight record survives the logged fold",
                    vec![Referent::new("sema-engine")],
                ),
            },
        ))
        .expect("seed first version eight record");
        live.assert(PreviousAssertion::new(
            records,
            SpiritStoreV8Record {
                record_identifier: String::from("t0tu"),
                entry: version_eight_previous_entry("migration is a logged fold", Vec::new()),
            },
        ))
        .expect("seed second version eight record");
        drop(live);

        let mut archive = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            archive_path,
            SPIRIT_STORE_V8_SCHEMA_VERSION,
        ))
        .expect("open version eight archive store");
        let archive_records = archive
            .register_table(PreviousTableDescriptor::new(RECORDS_TABLE))
            .expect("register version eight archive records table");
        archive
            .assert(PreviousAssertion::new(
                archive_records,
                SpiritStoreV8Record {
                    record_identifier: String::from("old1-3"),
                    entry: version_eight_previous_entry("archived record survives", Vec::new()),
                },
            ))
            .expect("seed version eight archived record");
    }

    fn seed_version_nine_current_store(live_path: &std::path::Path) {
        let mut live = CurrentSemaDatabase::open(
            CurrentEngineOpen::new(live_path, SPIRIT_STORE_V9_SCHEMA_VERSION).with_versioning(
                CurrentVersioningPolicy::new(CurrentVersionedStoreName::new("spirit:sema")),
            ),
        )
        .expect("open version nine current-layout store");
        let records = live
            .register_table(CurrentTableDescriptor::new(
                CURRENT_RECORDS_TABLE,
                CurrentFamilyName::new("RecordsFamily"),
                CurrentSchemaHash::new(SPIRIT_STORE_V9_RECORDS_FAMILY),
            ))
            .expect("register version nine records table");
        let _referents: super::CurrentTableReference<super::StoredReferent> = live
            .register_table(CurrentTableDescriptor::new(
                CURRENT_REFERENTS_TABLE,
                CurrentFamilyName::new("ReferentsFamily"),
                CurrentSchemaHash::new(SPIRIT_STORE_V9_REFERENTS_FAMILY),
            ))
            .expect("register version nine referents table");
        live.assert(CurrentAssertion::new(
            records,
            SpiritStoreV9Record {
                record_identifier: RecordIdentifier::new("qprc"),
                entry: store_version_nine::Entry::data_query_processing(
                    "version nine query planner record",
                    Certainty::new(Magnitude::High),
                    Importance::new(Magnitude::Medium),
                    Privacy::new(Magnitude::Zero),
                ),
            },
        ))
        .expect("seed version nine record");
    }

    /// The t0tu pilot witness: a version-8 store (no versioned log) migrates
    /// into a fresh version-9 store whose versioned log covers every migrated
    /// row, with identifiers unchanged, the typed migration marker present,
    /// and the archive sibling rebuilt under the current engine.
    #[test]
    fn migrates_version_eight_store_as_a_logged_fold() {
        let temporary = tempfile::tempdir().expect("create migration sandbox");
        let database_path = temporary.path().join("store.sema");
        let archive_path = temporary.path().join("store.archive.sema");
        seed_version_eight_store(&database_path, &archive_path);

        let output = StoreMigration::new(StoreMigrationRequest::new(
            database_path.display().to_string(),
        ))
        .run()
        .expect("run store migration");
        let StoreMigrationOutput::Migrated(completed) = output else {
            panic!("version eight store must migrate, got {output:?}");
        };
        assert_eq!(completed.record_count(), 2);
        assert_eq!(completed.referent_count(), 1);

        // The normal daemon query surface reads identical records with
        // identical identifiers.
        let migrated = Store::open(&database_path).expect("open migrated store");
        let first = migrated
            .entry_by_identifier("hj63")
            .expect("query first migrated entry")
            .expect("first migrated entry exists");
        assert_eq!(
            first.description.payload(),
            "version eight record survives the logged fold"
        );
        assert_eq!(
            first.referents.payload(),
            &vec![Referent::new("sema-engine")]
        );
        let second = migrated
            .entry_by_identifier("t0tu")
            .expect("query second migrated entry")
            .expect("second migrated entry exists");
        assert_eq!(second.description.payload(), "migration is a logged fold");

        // The versioned log covers every migrated row: one logged assert per
        // referent, record, and the marker, decodable through the generated
        // closed family sum back to the exact stored payloads.
        let log = migrated.versioned_log().expect("read versioned log");
        let operations: Vec<_> = log
            .iter()
            .flat_map(|entry| entry.operations().iter())
            .collect();
        assert_eq!(operations.len(), 4);
        let mut logged_record_identifiers = Vec::new();
        let mut logged_referent_count = 0;
        let mut logged_migrations = Vec::new();
        for operation in &operations {
            let payload = operation.payload().bytes().expect("assert payload bytes");
            match RecordFamily::decode(operation.family(), payload)
                .expect("decode logged family payload")
            {
                RecordFamily::RecordsFamily(record) => {
                    let stored = migrated
                        .entry_by_identifier(record.record_identifier.payload())
                        .expect("query logged record")
                        .expect("logged record exists in the store");
                    assert_eq!(stored, record.entry);
                    logged_record_identifiers.push(record.record_identifier.payload().clone());
                }
                RecordFamily::ReferentsFamily(referent) => {
                    assert_eq!(referent.referent, Referent::new("sema-engine"));
                    logged_referent_count += 1;
                }
                RecordFamily::MigrationsFamily(migration) => logged_migrations.push(migration),
            }
        }
        logged_record_identifiers.sort();
        assert_eq!(logged_record_identifiers, vec!["hj63", "t0tu"]);
        assert_eq!(logged_referent_count, 1);

        // The typed migration marker is logged AND materialized.
        assert_eq!(logged_migrations.len(), 1);
        let migrations = migrated.migrations().expect("read migration markers");
        assert_eq!(migrations.len(), 1);
        assert_eq!(*migrations[0].source_schema_version.payload(), 8);
        assert_eq!(*migrations[0].migrated_record_count.payload(), 2);
        assert_eq!(*migrations[0].migrated_referent_count.payload(), 1);
        assert_eq!(logged_migrations[0], migrations[0]);

        // The archive sibling was rebuilt under the current engine with its
        // archive keys preserved: retire into it to prove it accepts writes.
        let retired = migrated
            .retire(crate::schema::signal::Retirement {
                record_identifier: crate::schema::signal::RecordIdentifier::new("t0tu"),
                justification: crate::schema::signal::Justification {
                    testimony: crate::schema::signal::Testimony::new(vec![
                        crate::schema::signal::VerbatimQuote {
                            quote_text: crate::schema::signal::QuoteText::new(
                                "witness: the migrated archive accepts writes",
                            ),
                            antecedent: None,
                        },
                    ]),
                    reasoning: crate::schema::signal::Reasoning::new(
                        "migration witness retirement",
                    ),
                },
            })
            .expect("retire into migrated archive");
        assert!(retired.is_some());
    }

    /// The data-loss-protection witness on the real pilot: checkpoint the
    /// MIGRATED store, import checkpoint + suffix into a fresh store through
    /// the engine-owned import session, and the daemon-level query surface is
    /// identical — including the migration marker.
    #[test]
    fn migrated_store_checkpoint_restores_an_identical_fresh_store() {
        let temporary = tempfile::tempdir().expect("create migration sandbox");
        let database_path = temporary.path().join("store.sema");
        let archive_path = temporary.path().join("store.archive.sema");
        seed_version_eight_store(&database_path, &archive_path);
        StoreMigration::new(StoreMigrationRequest::new(
            database_path.display().to_string(),
        ))
        .run()
        .expect("run store migration");

        let migrated = Store::open(&database_path).expect("open migrated store");
        migrated.checkpoint().expect("checkpoint migrated store");
        // Post-checkpoint suffix: one more durable write rides the log.
        let suffix_receipt = migrated
            .record_entry(version_eight_entry(
                "suffix write after migration",
                Vec::new(),
            ))
            .expect("record suffix entry");
        let checkpoint = migrated
            .latest_checkpoint()
            .expect("load checkpoint")
            .expect("checkpoint exists");
        let suffix = migrated
            .versioned_log_from(checkpoint.metadata().covered().last().next())
            .expect("read log suffix");
        assert_eq!(suffix.len(), 1);

        let restored = Store::import(temporary.path().join("restored.sema"), checkpoint, suffix)
            .expect("import migrated checkpoint into fresh store");

        assert_eq!(restored.len(), migrated.len());
        for identifier in [
            "hj63",
            "t0tu",
            suffix_receipt.record_identifier.payload().as_str(),
        ] {
            assert_eq!(
                restored
                    .entry_by_identifier(identifier)
                    .expect("query restored entry"),
                migrated
                    .entry_by_identifier(identifier)
                    .expect("query migrated entry"),
                "restored entry differs for identifier {identifier}",
            );
        }
        assert_eq!(restored.database_marker(), migrated.database_marker());
        assert_eq!(
            restored.migrations().expect("restored migration markers"),
            migrated.migrations().expect("migrated migration markers"),
        );
    }

    #[test]
    fn migrates_version_nine_current_layout_domains_to_terminal_values_and_keywords() {
        let temporary = tempfile::tempdir().expect("create migration sandbox");
        let database_path = temporary.path().join("store.sema");
        seed_version_nine_current_store(&database_path);

        let output = StoreMigration::new(StoreMigrationRequest::new(
            database_path.display().to_string(),
        ))
        .run()
        .expect("run store migration");
        let StoreMigrationOutput::Migrated(completed) = output else {
            panic!("version nine store must migrate, got {output:?}");
        };
        assert_eq!(completed.record_count(), 1);
        assert_eq!(completed.referent_count(), 0);

        let migrated = Store::open(&database_path).expect("open migrated store");
        let entry = migrated
            .entry_by_identifier("qprc")
            .expect("query migrated v9 entry")
            .expect("migrated v9 entry exists");
        assert_eq!(
            entry.domains,
            Domains::new(vec![Domain::Technology(Technology::Software(
                Software::Data(None)
            ))])
        );
        assert!(
            entry.description.payload().contains("*query processing*"),
            "cut leaf specificity must survive as a keyword, got {}",
            entry.description.payload()
        );
        let migrations = migrated.migrations().expect("read migration markers");
        assert_eq!(migrations.len(), 1);
        assert_eq!(*migrations[0].source_schema_version.payload(), 9);
    }

    /// A second migration run over an already-migrated store reports
    /// `Current` and rewrites nothing.
    #[test]
    fn second_migration_run_is_a_current_no_op() {
        let temporary = tempfile::tempdir().expect("create migration sandbox");
        let database_path = temporary.path().join("store.sema");
        let archive_path = temporary.path().join("store.archive.sema");
        seed_version_eight_store(&database_path, &archive_path);
        let request = StoreMigrationRequest::new(database_path.display().to_string());

        let first = StoreMigration::new(request.clone())
            .run()
            .expect("first migration run");
        assert!(matches!(first, StoreMigrationOutput::Migrated(_)));
        let second = StoreMigration::new(request)
            .run()
            .expect("second migration run");
        let StoreMigrationOutput::Current(completed) = second else {
            panic!("already-migrated store must report Current, got {second:?}");
        };
        assert_eq!(completed.record_count(), 2);
    }

    /// The 12r5 witness: a migration run sweeps EVERY stale
    /// `*.schema-10-migrating-*.sema` temporary beside the live store, not just
    /// this process's PID-suffixed one. A temporary left by a crash under a
    /// different PID would otherwise linger forever.
    #[test]
    fn migration_run_sweeps_a_foreign_pid_stale_temporary() {
        let temporary = tempfile::tempdir().expect("create migration sandbox");
        let database_path = temporary.path().join("store.sema");
        let archive_path = temporary.path().join("store.archive.sema");
        seed_version_eight_store(&database_path, &archive_path);

        // A stale temporary from a crashed run under a different PID, plus a
        // matching archive temporary; their suffixes are not this process's.
        let stale_live = temporary
            .path()
            .join("store.schema-10-migrating-424242.sema");
        let stale_archive = temporary
            .path()
            .join("store.archive.schema-10-migrating-424242.sema");
        std::fs::write(&stale_live, b"stale live temporary").expect("seed stale live temporary");
        std::fs::write(&stale_archive, b"stale archive temporary")
            .expect("seed stale archive temporary");

        let output = StoreMigration::new(StoreMigrationRequest::new(
            database_path.display().to_string(),
        ))
        .run()
        .expect("run store migration over a foreign-PID stale temporary");
        assert!(matches!(output, StoreMigrationOutput::Migrated(_)));
        assert!(
            !stale_live.exists(),
            "foreign-PID stale live temporary must be swept on re-run",
        );
        assert!(
            !stale_archive.exists(),
            "foreign-PID stale archive temporary must be swept on re-run",
        );
    }

    #[test]
    fn upgrades_version_seven_domains_into_software_branch() {
        let temporary = tempfile::tempdir().expect("create upgrade sandbox");
        let database_path = temporary.path().join("store.sema");

        let mut version_seven_database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            &database_path,
            SPIRIT_STORE_V7_SCHEMA_VERSION,
        ))
        .expect("open version seven database");
        let records = version_seven_database
            .register_table(PreviousTableDescriptor::new(RECORDS_TABLE))
            .expect("register version seven records table");
        version_seven_database
            .assert(PreviousAssertion::new(
                records,
                SpiritStoreV7Record {
                    record_identifier: String::from("0001"),
                    entry: SpiritStoreV7Entry {
                        domains: store_version_seven::Domains(vec![
                            store_version_seven::Domain::Craft(store_version_seven::Craft::Schema),
                            store_version_seven::Domain::Craft(
                                store_version_seven::Craft::Infrastructure,
                            ),
                            store_version_seven::Domain::Information(
                                store_version_seven::Information::Documentation,
                            ),
                        ]),
                        kind: crate::schema::signal::Kind::Decision,
                        description: crate::schema::signal::Description::new(
                            "version seven record survives upgrade",
                        ),
                        certainty: crate::schema::signal::Certainty::new(Magnitude::High),
                        importance: crate::schema::signal::Importance::new(Magnitude::Medium),
                        privacy: crate::schema::signal::Privacy::new(Magnitude::Zero),
                        referents: crate::schema::signal::Referents::new(Vec::new()),
                    },
                },
            ))
            .expect("seed version seven record");
        drop(version_seven_database);

        let output = StoreMigration::new(StoreMigrationRequest::new(
            database_path.display().to_string(),
        ))
        .run()
        .expect("run store migration");
        let target_store = Store::open(database_path).expect("open upgraded store");
        let migrated_entry = target_store
            .entry_by_identifier("0001")
            .expect("query upgraded entry")
            .expect("upgraded entry exists");

        assert!(matches!(output, StoreMigrationOutput::Migrated(_)));
        assert_eq!(
            migrated_entry.domains,
            Domains::new(vec![
                Domain::Technology(Technology::Software(Software::Data(Some(
                    DataLeaf::SchemaEvolution
                )))),
                Domain::Technology(Technology::Software(Software::Operations(Some(
                    OperationsLeaf::Deployment
                )))),
                Domain::Information(Information::Documentation),
            ])
        );
        let migrations = target_store.migrations().expect("read migration markers");
        assert_eq!(migrations.len(), 1);
        assert_eq!(*migrations[0].source_schema_version.payload(), 7);
    }

    /// The im1l witness: a version-7 store whose record carries a non-empty
    /// referent migrates cleanly because the v7 open now also reads the v7
    /// referents table. Before this fix the v7 open registered only the
    /// records table, so the referent never landed in the fresh store and the
    /// record import aborted with `UnregisteredReferent`.
    #[test]
    fn migrates_version_seven_record_carrying_a_registered_referent() {
        let temporary = tempfile::tempdir().expect("create migration sandbox");
        let database_path = temporary.path().join("store.sema");

        let mut version_seven_database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            &database_path,
            SPIRIT_STORE_V7_SCHEMA_VERSION,
        ))
        .expect("open version seven database");
        let records = version_seven_database
            .register_table(PreviousTableDescriptor::new(RECORDS_TABLE))
            .expect("register version seven records table");
        let referents = version_seven_database
            .register_table(PreviousTableDescriptor::new(REFERENTS_TABLE))
            .expect("register version seven referents table");
        version_seven_database
            .assert(PreviousAssertion::new(
                referents,
                SpiritStoreV7Referent {
                    referent: Referent::new("sema-engine"),
                    aliases: Referents::new(vec![Referent::new("sema engine")]),
                },
            ))
            .expect("seed version seven referent");
        version_seven_database
            .assert(PreviousAssertion::new(
                records,
                SpiritStoreV7Record {
                    record_identifier: String::from("ref7"),
                    entry: SpiritStoreV7Entry {
                        domains: store_version_seven::Domains(vec![
                            store_version_seven::Domain::Information(
                                store_version_seven::Information::Documentation,
                            ),
                        ]),
                        kind: Kind::Decision,
                        description: Description::new(
                            "version seven record referencing a registered referent",
                        ),
                        certainty: Certainty::new(Magnitude::High),
                        importance: Importance::new(Magnitude::Medium),
                        privacy: Privacy::new(Magnitude::Zero),
                        referents: Referents::new(vec![Referent::new("sema-engine")]),
                    },
                },
            ))
            .expect("seed version seven record carrying a referent");
        drop(version_seven_database);

        let output = StoreMigration::new(StoreMigrationRequest::new(
            database_path.display().to_string(),
        ))
        .run()
        .expect("version seven referent-carrying store migrates cleanly");
        let StoreMigrationOutput::Migrated(completed) = output else {
            panic!("version seven store must migrate, got {output:?}");
        };
        assert_eq!(completed.record_count(), 1);
        assert_eq!(completed.referent_count(), 1);

        let migrated = Store::open(&database_path).expect("open migrated store");
        let entry = migrated
            .entry_by_identifier("ref7")
            .expect("query migrated entry")
            .expect("migrated entry exists");
        assert_eq!(
            entry.referents.payload(),
            &vec![Referent::new("sema-engine")],
            "the record's referent survives the fold",
        );
        let migrations = migrated.migrations().expect("read migration markers");
        assert_eq!(migrations.len(), 1);
        assert_eq!(*migrations[0].source_schema_version.payload(), 7);
        assert_eq!(*migrations[0].migrated_referent_count.payload(), 1);
    }
}
