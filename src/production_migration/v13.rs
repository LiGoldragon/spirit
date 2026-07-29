//! Frozen readers for Spirit schema version 13.
//!
//! These types reproduce the archived Rust layouts emitted by the published
//! v13 Schema toolchain. They are migration evidence, not a compatibility
//! contract: the module is reachable only through the `production-migration`
//! feature, and no daemon or ordinary/meta client path imports it.
//!
//! Provenance:
//! - Spirit: `e0d7ec1716b3f1e280eeb0d8853f26e70aa07a65`
//! - signal-spirit: `1cf7c010029de46369b742687da4fa1ca6def9a9`
//! - signal-domain: `801e1c5bcc824c9760e246205826e3c8e962d005`
//!
//! The final fold into Ethos-generated current families is deliberately not
//! implemented here because those current family types do not exist yet.
//! [`crate::production_migration::v13::FoldSink`] is the typed handoff point
//! for that later migration.

use std::path::{Path, PathBuf};

use sema_engine::{
    Engine as SemaDatabase, EngineOpen, EngineRecord, FamilyIdentity, FamilyName, QueryPlan,
    RecordKey, SchemaHash, SchemaVersion, TableDescriptor, TableName, TableReference,
    VersionedStoreName, VersioningPolicy,
};
use thiserror::Error;

pub const SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(13);
pub const STORE_NAME: &str = "spirit:sema";

pub const RECORDS_FAMILY_SCHEMA_HASH: [u8; 32] = [
    169, 167, 27, 203, 113, 158, 12, 113, 89, 93, 195, 166, 134, 208, 34, 40, 178, 38, 203, 139,
    155, 209, 108, 101, 12, 183, 180, 233, 6, 84, 230, 177,
];
pub const REFERENTS_FAMILY_SCHEMA_HASH: [u8; 32] = [
    104, 195, 227, 181, 142, 254, 234, 107, 128, 177, 214, 13, 194, 75, 25, 253, 10, 38, 233, 129,
    32, 48, 247, 79, 147, 203, 30, 52, 248, 158, 148, 249,
];
pub const MIGRATIONS_FAMILY_SCHEMA_HASH: [u8; 32] = [
    162, 255, 229, 245, 220, 189, 118, 68, 34, 109, 161, 159, 253, 62, 185, 124, 148, 130, 225,
    124, 212, 69, 80, 89, 118, 12, 161, 139, 156, 198, 112, 174,
];

macro_rules! archived_unit_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(
            rkyv::Archive,
            rkyv::Serialize,
            rkyv::Deserialize,
            Clone,
            Copy,
            Debug,
            PartialEq,
            Eq,
        )]
        pub enum $name {
            $($variant),+
        }
    };
}

archived_unit_enum!(Health {
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
});
archived_unit_enum!(Food {
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
});
archived_unit_enum!(Home {
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
});
archived_unit_enum!(Finance {
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
});
archived_unit_enum!(Work {
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
});
archived_unit_enum!(Craft {
    Electronics,
    Construction,
    Carpentry,
    Metalworking,
    Sewing,
    Manufacturing,
    Repair,
    Engineering,
    Handicraft,
    Invention,
});
archived_unit_enum!(Knowledge {
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
});
archived_unit_enum!(Education {
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
});
archived_unit_enum!(Language {
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
});
archived_unit_enum!(Art {
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
});
archived_unit_enum!(Kinship {
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
});
archived_unit_enum!(Selfhood {
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
});
archived_unit_enum!(Spirituality {
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
});
archived_unit_enum!(Governance {
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
});
archived_unit_enum!(Law {
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
});
archived_unit_enum!(Community {
    Neighborliness,
    Volunteering,
    Solidarity,
    Membership,
    Gatherings,
    Reputation,
    Service,
    Hospitality,
    Institutions,
});
archived_unit_enum!(Nature {
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
});
archived_unit_enum!(Travel {
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
});
archived_unit_enum!(Commerce {
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
});
archived_unit_enum!(Leisure {
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
});
archived_unit_enum!(Appearance {
    Clothing,
    Grooming,
    Style,
    Cosmetics,
    Etiquette,
    Comportment,
});
archived_unit_enum!(Safety {
    Protection,
    Preparedness,
    Risk,
    Cybersecurity,
    Privacy,
    Disaster,
    Military,
    Deterrence,
});
archived_unit_enum!(Information {
    Curation,
    RecordKeeping,
    Documentation,
    News,
    Broadcasting,
    Archives,
    Database,
    Retrieval,
    Classification,
});
archived_unit_enum!(HardwareLeaf { All, Networking });
archived_unit_enum!(ProgrammingLeaf {
    All,
    TypeSystems,
    Compilation,
    Parsing,
    Grammars,
    CodeGeneration,
    Metaprogramming,
    Macros,
    DomainSpecificLanguages,
});
archived_unit_enum!(SystemsLeaf {
    All,
    SystemsProgramming,
    Concurrency,
});
archived_unit_enum!(DistributedLeaf {
    All,
    ProtocolDesign,
    EventDrivenArchitecture,
});
archived_unit_enum!(DataLeaf {
    All,
    Persistence,
    Serialization,
    Formats,
    Modeling,
    SchemaEvolution,
    Migration,
});
archived_unit_enum!(IntelligenceLeaf { All, AgentSystems });
archived_unit_enum!(SecurityLeaf {
    All,
    Cryptography,
    Authentication,
    Authorization,
    SecretsManagement,
    Privacy,
});
archived_unit_enum!(QualityLeaf { All, Testing });
archived_unit_enum!(OperationsLeaf {
    All,
    BuildSystem,
    ReleaseEngineering,
    DependencyManagement,
    Deployment,
    ConfigurationManagement,
});
archived_unit_enum!(ObservabilityLeaf { All, Tracing });
archived_unit_enum!(SurfacesLeaf {
    All,
    Visualization,
    CommandLineInterfaces,
});
archived_unit_enum!(EngineeringLeaf {
    All,
    Architecture,
    Design,
    ApplicationProgrammingInterfaces,
    Documentation,
    VersionControl,
    DevelopmentProcess,
    Management,
    Modularity,
});

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Domain {
    All,
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

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Technology {
    Hardware(HardwareLeaf),
    Software(Software),
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Software {
    Programming(ProgrammingLeaf),
    Theory,
    Systems(SystemsLeaf),
    Distributed(DistributedLeaf),
    Data(DataLeaf),
    Intelligence(IntelligenceLeaf),
    Security(SecurityLeaf),
    Quality(QualityLeaf),
    Operations(OperationsLeaf),
    Observability(ObservabilityLeaf),
    Surfaces(SurfacesLeaf),
    Engineering(EngineeringLeaf),
}

archived_unit_enum!(Kind {
    Decision,
    Principle,
    Correction,
    Clarification,
    Constraint,
});
archived_unit_enum!(Magnitude {
    Zero,
    Minimum,
    VeryLow,
    Low,
    Medium,
    High,
    VeryHigh,
    Maximum,
});

macro_rules! archived_newtype {
    ($name:ident($inner:ty)) => {
        #[derive(
            rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq,
        )]
        pub struct $name($inner);

        impl $name {
            pub fn new(value: $inner) -> Self {
                Self(value)
            }

            pub fn payload(&self) -> &$inner {
                &self.0
            }

            pub fn into_payload(self) -> $inner {
                self.0
            }
        }
    };
}

archived_newtype!(Domains(Vec<Domain>));
archived_newtype!(Referent(String));
archived_newtype!(Referents(Vec<Referent>));
archived_newtype!(Aliases(Referents));
archived_newtype!(Description(String));
archived_newtype!(RecordIdentifier(String));
archived_newtype!(Privacy(Magnitude));
archived_newtype!(Certainty(Magnitude));
archived_newtype!(Importance(Magnitude));
archived_newtype!(SourceSchemaVersion(u64));
archived_newtype!(MigratedRecordCount(u64));
archived_newtype!(MigratedReferentCount(u64));

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub domains: Domains,
    pub kind: Kind,
    pub description: Description,
    pub certainty: Certainty,
    pub importance: Importance,
    pub privacy: Privacy,
    pub referents: Referents,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredRecord {
    pub record_identifier: RecordIdentifier,
    pub entry: Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredReferent {
    pub referent: Referent,
    pub aliases: Aliases,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    pub source_schema_version: SourceSchemaVersion,
    pub migrated_record_count: MigratedRecordCount,
    pub migrated_referent_count: MigratedReferentCount,
}

impl EngineRecord for StoredRecord {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.record_identifier.payload().clone())
    }
}

impl EngineRecord for StoredReferent {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.referent.payload().clone())
    }
}

impl EngineRecord for Migration {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(format!(
            "from-schema-{}",
            self.source_schema_version.payload()
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Records,
    Referents,
    Migrations,
}

impl std::fmt::Display for Family {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Records => "records",
            Self::Referents => "referents",
            Self::Migrations => "migrations",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSurface {
    Live,
    RecordsOnlyArchive,
}

impl std::fmt::Display for CatalogSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Live => "live",
            Self::RecordsOnlyArchive => "records-only archive",
        })
    }
}

#[derive(Debug, Error)]
pub enum ReaderError {
    #[error("open the frozen v13 store: {0}")]
    Open(#[source] sema_engine::Error),
    #[error("v13 {surface} catalog differs: expected {expected:?}, found {found:?}")]
    Catalog {
        surface: CatalogSurface,
        expected: Vec<FamilyIdentity>,
        found: Vec<FamilyIdentity>,
    },
    #[error("enumerate frozen v13 {family} family: {source}")]
    Enumerate {
        family: Family,
        #[source]
        source: sema_engine::Error,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    pub records: Vec<StoredRecord>,
    pub referents: Vec<StoredReferent>,
    pub migrations: Vec<Migration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldReceipt {
    pub record_count: usize,
    pub referent_count: usize,
    pub migration_count: usize,
}

pub trait FoldSink {
    type Error;

    fn accept_record(&mut self, record: StoredRecord) -> Result<(), Self::Error>;
    fn accept_referent(&mut self, referent: StoredReferent) -> Result<(), Self::Error>;
    fn accept_migration(&mut self, migration: Migration) -> Result<(), Self::Error>;
}

#[derive(Debug, Error)]
pub enum FoldError<SinkError>
where
    SinkError: std::error::Error + 'static,
{
    #[error(transparent)]
    Read(#[from] ReaderError),
    #[error("v13 fold sink refused {family}: {source}")]
    Sink {
        family: Family,
        #[source]
        source: SinkError,
    },
}

pub struct LiveReader {
    database: SemaDatabase,
    records: TableReference<StoredRecord>,
    referents: TableReference<StoredReferent>,
    migrations: TableReference<Migration>,
    path: PathBuf,
}

pub struct ArchiveReader {
    database: SemaDatabase,
    records: TableReference<StoredRecord>,
    path: PathBuf,
}

pub struct FrozenLayout {
    schema_version: SchemaVersion,
}

impl FrozenLayout {
    pub fn version_thirteen() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
        }
    }

    pub fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    pub fn records_descriptor(&self) -> TableDescriptor<StoredRecord> {
        TableDescriptor::new(
            TableName::new("records"),
            FamilyName::new("RecordsFamily"),
            SchemaHash::new(RECORDS_FAMILY_SCHEMA_HASH),
        )
    }

    pub fn referents_descriptor(&self) -> TableDescriptor<StoredReferent> {
        TableDescriptor::new(
            TableName::new("referents"),
            FamilyName::new("ReferentsFamily"),
            SchemaHash::new(REFERENTS_FAMILY_SCHEMA_HASH),
        )
    }

    pub fn migrations_descriptor(&self) -> TableDescriptor<Migration> {
        TableDescriptor::new(
            TableName::new("migrations"),
            FamilyName::new("MigrationsFamily"),
            SchemaHash::new(MIGRATIONS_FAMILY_SCHEMA_HASH),
        )
    }

    fn versioning_policy(&self) -> VersioningPolicy {
        VersioningPolicy::new(VersionedStoreName::new(STORE_NAME))
    }

    fn records_identity(&self) -> FamilyIdentity {
        self.records_descriptor().family_identity()
    }

    fn live_identities(&self) -> [FamilyIdentity; 3] {
        [
            self.records_identity(),
            self.referents_descriptor().family_identity(),
            self.migrations_descriptor().family_identity(),
        ]
    }

    fn validate_catalog(
        &self,
        database: &SemaDatabase,
        surface: CatalogSurface,
        expected: &[FamilyIdentity],
    ) -> Result<(), ReaderError> {
        let found = database
            .list_tables()
            .iter()
            .map(|registration| registration.identity().clone())
            .collect::<Vec<_>>();
        if found.len() == expected.len() && expected.iter().all(|identity| found.contains(identity))
        {
            return Ok(());
        }
        Err(ReaderError::Catalog {
            surface,
            expected: expected.to_vec(),
            found,
        })
    }
}

impl LiveReader {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ReaderError> {
        let path = path.into();
        let layout = FrozenLayout::version_thirteen();
        let database = SemaDatabase::open(
            EngineOpen::new(path.clone(), layout.schema_version())
                .with_versioning(layout.versioning_policy()),
        )
        .map_err(ReaderError::Open)?;
        layout.validate_catalog(&database, CatalogSurface::Live, &layout.live_identities())?;
        Ok(Self {
            database,
            records: TableReference::new(TableName::new("records")),
            referents: TableReference::new(TableName::new("referents")),
            migrations: TableReference::new(TableName::new("migrations")),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn enumerate(&self) -> Result<Inventory, ReaderError> {
        let mut records = self
            .database
            .match_records(QueryPlan::all(self.records))
            .map_err(|source| ReaderError::Enumerate {
                family: Family::Records,
                source,
            })?
            .records()
            .to_vec();
        let mut referents = self
            .database
            .match_records(QueryPlan::all(self.referents))
            .map_err(|source| ReaderError::Enumerate {
                family: Family::Referents,
                source,
            })?
            .records()
            .to_vec();
        let mut migrations = self
            .database
            .match_records(QueryPlan::all(self.migrations))
            .map_err(|source| ReaderError::Enumerate {
                family: Family::Migrations,
                source,
            })?
            .records()
            .to_vec();

        records.sort_by(|left, right| {
            left.record_identifier
                .payload()
                .cmp(right.record_identifier.payload())
        });
        referents.sort_by(|left, right| left.referent.payload().cmp(right.referent.payload()));
        migrations.sort_by_key(|migration| *migration.source_schema_version.payload());

        Ok(Inventory {
            records,
            referents,
            migrations,
        })
    }

    pub fn fold_into<Sink>(&self, sink: &mut Sink) -> Result<FoldReceipt, FoldError<Sink::Error>>
    where
        Sink: FoldSink,
        Sink::Error: std::error::Error + 'static,
    {
        let inventory = self.enumerate()?;
        let receipt = FoldReceipt {
            record_count: inventory.records.len(),
            referent_count: inventory.referents.len(),
            migration_count: inventory.migrations.len(),
        };
        for record in inventory.records {
            sink.accept_record(record)
                .map_err(|source| FoldError::Sink {
                    family: Family::Records,
                    source,
                })?;
        }
        for referent in inventory.referents {
            sink.accept_referent(referent)
                .map_err(|source| FoldError::Sink {
                    family: Family::Referents,
                    source,
                })?;
        }
        for migration in inventory.migrations {
            sink.accept_migration(migration)
                .map_err(|source| FoldError::Sink {
                    family: Family::Migrations,
                    source,
                })?;
        }
        Ok(receipt)
    }
}

impl ArchiveReader {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ReaderError> {
        let path = path.into();
        let layout = FrozenLayout::version_thirteen();
        let database = SemaDatabase::open(EngineOpen::new(path.clone(), layout.schema_version()))
            .map_err(ReaderError::Open)?;
        layout.validate_catalog(
            &database,
            CatalogSurface::RecordsOnlyArchive,
            &[layout.records_identity()],
        )?;
        Ok(Self {
            database,
            records: TableReference::new(TableName::new("records")),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn records(&self) -> Result<Vec<StoredRecord>, ReaderError> {
        let mut records = self
            .database
            .match_records(QueryPlan::all(self.records))
            .map_err(|source| ReaderError::Enumerate {
                family: Family::Records,
                source,
            })?
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

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, mem};

    use sema_engine::{Assertion, Engine as SemaDatabase, EngineOpen};
    use tempfile::tempdir;

    use super::*;

    fn sample_entry() -> Entry {
        Entry {
            domains: Domains::new(vec![
                Domain::All,
                Domain::Technology(Technology::Software(Software::Data(
                    DataLeaf::SchemaEvolution,
                ))),
            ]),
            kind: Kind::Constraint,
            description: Description::new("v13 frozen".to_owned()),
            certainty: Certainty::new(Magnitude::Maximum),
            importance: Importance::new(Magnitude::High),
            privacy: Privacy::new(Magnitude::Zero),
            referents: Referents::new(vec![Referent::new("psyche".to_owned())]),
        }
    }

    fn sample_record(identifier: &str) -> StoredRecord {
        StoredRecord {
            record_identifier: RecordIdentifier::new(identifier.to_owned()),
            entry: sample_entry(),
        }
    }

    fn sample_referent() -> StoredReferent {
        StoredReferent {
            referent: Referent::new("psyche".to_owned()),
            aliases: Aliases::new(Referents::new(vec![Referent::new("li".to_owned())])),
        }
    }

    fn sample_migration() -> Migration {
        Migration {
            source_schema_version: SourceSchemaVersion::new(12),
            migrated_record_count: MigratedRecordCount::new(2),
            migrated_referent_count: MigratedReferentCount::new(1),
        }
    }

    fn seed_live(path: &Path) {
        let layout = FrozenLayout::version_thirteen();
        let mut database = SemaDatabase::open(
            EngineOpen::new(path, layout.schema_version())
                .with_versioning(layout.versioning_policy()),
        )
        .expect("open synthetic v13 live store");
        let records = database
            .register_table(layout.records_descriptor())
            .expect("register records");
        let referents = database
            .register_table(layout.referents_descriptor())
            .expect("register referents");
        let migrations = database
            .register_table(layout.migrations_descriptor())
            .expect("register migrations");
        database
            .assert(Assertion::new(records, sample_record("7z")))
            .expect("assert first record");
        database
            .assert(Assertion::new(records, sample_record("a1")))
            .expect("assert second record");
        database
            .assert(Assertion::new(referents, sample_referent()))
            .expect("assert referent");
        database
            .assert(Assertion::new(migrations, sample_migration()))
            .expect("assert migration marker");
    }

    fn seed_archive(path: &Path) {
        let layout = FrozenLayout::version_thirteen();
        let mut database = SemaDatabase::open(EngineOpen::new(path, layout.schema_version()))
            .expect("open v13 archive");
        let records = database
            .register_table(layout.records_descriptor())
            .expect("register archive records");
        database
            .assert(Assertion::new(records, sample_record("7z:1")))
            .expect("assert archived record");
    }

    fn hex_bytes(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn archived_hex<Value>(value: &Value) -> String
    where
        Value: rkyv::Archive
            + for<'serialize> rkyv::Serialize<
                rkyv::rancor::Strategy<
                    rkyv::ser::Serializer<
                        rkyv::util::AlignedVec,
                        rkyv::ser::allocator::ArenaHandle<'serialize>,
                        rkyv::ser::sharing::Share,
                    >,
                    rkyv::rancor::Error,
                >,
            >,
    {
        hex_bytes(
            rkyv::to_bytes::<rkyv::rancor::Error>(value)
                .expect("archive frozen v13 value")
                .as_slice(),
        )
    }

    #[test]
    fn archived_layout_and_discriminants_are_locked() {
        let record =
            rkyv::to_bytes::<rkyv::rancor::Error>(&sample_record("7z")).expect("archive record");
        let referent =
            rkyv::to_bytes::<rkyv::rancor::Error>(&sample_referent()).expect("archive referent");
        let migration =
            rkyv::to_bytes::<rkyv::rancor::Error>(&sample_migration()).expect("archive migration");

        assert_eq!(
            hex_bytes(record.as_slice()),
            "00000000180104057631332066726f7a656e707379636865ffff377affffffffffffdeffffff02000000048a000000ddffffff070500dcffffff01000000"
        );
        assert_eq!(
            hex_bytes(referent.as_slice()),
            "6c69ffffffffffff707379636865fffff0ffffff01000000"
        );
        assert_eq!(
            hex_bytes(migration.as_slice()),
            "0c0000000000000002000000000000000100000000000000"
        );
        assert_eq!(archived_hex(&Domain::All), "00000000");
        assert_eq!(
            archived_hex(&Domain::Technology(Technology::Software(
                Software::Engineering(EngineeringLeaf::Modularity)
            ))),
            "18010b08"
        );
        assert_eq!(
            [
                archived_hex(&Kind::Decision),
                archived_hex(&Kind::Principle),
                archived_hex(&Kind::Correction),
                archived_hex(&Kind::Clarification),
                archived_hex(&Kind::Constraint),
            ],
            ["00", "01", "02", "03", "04"]
        );
        assert_eq!(
            [
                archived_hex(&Magnitude::Zero),
                archived_hex(&Magnitude::Minimum),
                archived_hex(&Magnitude::VeryLow),
                archived_hex(&Magnitude::Low),
                archived_hex(&Magnitude::Medium),
                archived_hex(&Magnitude::High),
                archived_hex(&Magnitude::VeryHigh),
                archived_hex(&Magnitude::Maximum),
            ],
            ["00", "01", "02", "03", "04", "05", "06", "07"]
        );
        assert_eq!(
            (
                mem::size_of::<ArchivedStoredRecord>(),
                mem::align_of::<ArchivedStoredRecord>(),
                mem::size_of::<ArchivedStoredReferent>(),
                mem::align_of::<ArchivedStoredReferent>(),
                mem::size_of::<ArchivedMigration>(),
                mem::align_of::<ArchivedMigration>(),
                mem::size_of::<ArchivedDomain>(),
                mem::align_of::<ArchivedDomain>(),
            ),
            (36, 1, 16, 1, 24, 1, 4, 1)
        );
    }

    #[test]
    fn live_enumeration_preserves_identifiers_and_reruns_without_source_writes() {
        let temporary = tempdir().expect("temporary directory");
        let path = temporary.path().join("spirit-v13.sema");
        seed_live(&path);
        let first_reader = LiveReader::open(&path).expect("open frozen reader");
        let before_marker = first_reader
            .database
            .current_database_marker()
            .expect("read source marker");
        let before_catalog = first_reader.database.list_tables();
        let first = first_reader.enumerate().expect("enumerate frozen rows");
        drop(first_reader);

        let second_reader = LiveReader::open(&path).expect("reopen frozen reader");
        let second = second_reader
            .enumerate()
            .expect("enumerate frozen rows again");

        assert_eq!(first, second);
        assert_eq!(
            first
                .records
                .iter()
                .map(|record| record.record_identifier.payload().as_str())
                .collect::<Vec<_>>(),
            ["7z", "a1"]
        );
        assert_eq!(
            second_reader
                .database
                .current_database_marker()
                .expect("read source marker again"),
            before_marker
        );
        assert_eq!(second_reader.database.list_tables(), before_catalog);
    }

    #[test]
    fn records_only_archive_reader_does_not_register_live_families() {
        let temporary = tempdir().expect("temporary directory");
        let path = temporary.path().join("spirit-v13.archive.sema");
        seed_archive(&path);
        let reader = ArchiveReader::open(&path).expect("open records-only archive");
        let before_catalog = reader.database.list_tables();
        let records = reader.records().expect("enumerate archive records");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_identifier.payload(), "7z:1");
        assert_eq!(before_catalog.len(), 1);
        assert_eq!(
            before_catalog[0].identity(),
            &FrozenLayout::version_thirteen()
                .records_descriptor()
                .family_identity()
        );
        drop(reader);

        let reopened = ArchiveReader::open(&path).expect("reopen records-only archive");
        assert_eq!(reopened.records().expect("enumerate again"), records);
        assert_eq!(reopened.database.list_tables(), before_catalog);
    }

    #[test]
    fn wrong_version_and_corrupt_family_leave_sources_unchanged() {
        let temporary = tempdir().expect("temporary directory");
        let layout = FrozenLayout::version_thirteen();
        let wrong_version = temporary.path().join("wrong-version.sema");
        drop(
            SemaDatabase::open(
                EngineOpen::new(&wrong_version, SchemaVersion::new(12))
                    .with_versioning(layout.versioning_policy()),
            )
            .expect("seed wrong-version store"),
        );
        let wrong_before = SemaDatabase::open(
            EngineOpen::new(&wrong_version, SchemaVersion::new(12))
                .with_versioning(layout.versioning_policy()),
        )
        .expect("reopen wrong-version source");
        let wrong_before_catalog = wrong_before.list_tables();
        let wrong_before_marker = wrong_before
            .current_database_marker()
            .expect("read wrong-version marker");
        drop(wrong_before);
        assert!(matches!(
            LiveReader::open(&wrong_version),
            Err(ReaderError::Open(_))
        ));
        let wrong_after = SemaDatabase::open(
            EngineOpen::new(&wrong_version, SchemaVersion::new(12))
                .with_versioning(layout.versioning_policy()),
        )
        .expect("reopen rejected wrong-version source");
        assert_eq!(wrong_after.list_tables(), wrong_before_catalog);
        assert_eq!(
            wrong_after
                .current_database_marker()
                .expect("read wrong-version marker again"),
            wrong_before_marker
        );

        let corrupt = temporary.path().join("corrupt-family.sema");
        let mut database = SemaDatabase::open(
            EngineOpen::new(&corrupt, layout.schema_version())
                .with_versioning(layout.versioning_policy()),
        )
        .expect("seed corrupt-catalog store");
        database
            .register_table(TableDescriptor::<StoredRecord>::new(
                TableName::new("records"),
                FamilyName::new("RecordsFamily"),
                SchemaHash::new([0; 32]),
            ))
            .expect("register corrupt record family");
        database
            .register_table(layout.referents_descriptor())
            .expect("register referents");
        database
            .register_table(layout.migrations_descriptor())
            .expect("register migrations");
        drop(database);
        let corrupt_before = SemaDatabase::open(
            EngineOpen::new(&corrupt, layout.schema_version())
                .with_versioning(layout.versioning_policy()),
        )
        .expect("reopen corrupt-catalog source");
        let corrupt_before_catalog = corrupt_before.list_tables();
        let corrupt_before_marker = corrupt_before
            .current_database_marker()
            .expect("read corrupt-catalog marker");
        drop(corrupt_before);
        assert!(matches!(
            LiveReader::open(&corrupt),
            Err(ReaderError::Catalog { .. })
        ));
        let corrupt_after = SemaDatabase::open(
            EngineOpen::new(&corrupt, layout.schema_version())
                .with_versioning(layout.versioning_policy()),
        )
        .expect("reopen rejected corrupt-catalog source");
        assert_eq!(corrupt_after.list_tables(), corrupt_before_catalog);
        assert_eq!(
            corrupt_after
                .current_database_marker()
                .expect("read corrupt-catalog marker again"),
            corrupt_before_marker
        );
    }

    #[derive(Debug, Error)]
    #[error("synthetic destination refused the first referent")]
    struct PartialDestination;

    #[derive(Default)]
    struct FailingSink {
        records: Vec<StoredRecord>,
    }

    impl FoldSink for FailingSink {
        type Error = PartialDestination;

        fn accept_record(&mut self, record: StoredRecord) -> Result<(), Self::Error> {
            self.records.push(record);
            Ok(())
        }

        fn accept_referent(&mut self, _referent: StoredReferent) -> Result<(), Self::Error> {
            Err(PartialDestination)
        }

        fn accept_migration(&mut self, _migration: Migration) -> Result<(), Self::Error> {
            unreachable!("the failing referent precedes migration delivery")
        }
    }

    #[derive(Default)]
    struct CollectingSink {
        inventory: Inventory,
    }

    impl FoldSink for CollectingSink {
        type Error = Infallible;

        fn accept_record(&mut self, record: StoredRecord) -> Result<(), Self::Error> {
            self.inventory.records.push(record);
            Ok(())
        }

        fn accept_referent(&mut self, referent: StoredReferent) -> Result<(), Self::Error> {
            self.inventory.referents.push(referent);
            Ok(())
        }

        fn accept_migration(&mut self, migration: Migration) -> Result<(), Self::Error> {
            self.inventory.migrations.push(migration);
            Ok(())
        }
    }

    #[test]
    fn partial_destination_and_successful_rerun_leave_source_unchanged() {
        let temporary = tempdir().expect("temporary directory");
        let path = temporary.path().join("partial-destination.sema");
        seed_live(&path);
        let reader = LiveReader::open(&path).expect("open frozen reader");
        let before_inventory = reader.enumerate().expect("read source inventory");
        let before_catalog = reader.database.list_tables();
        let before_marker = reader
            .database
            .current_database_marker()
            .expect("read source marker");

        let mut failing = FailingSink::default();
        assert!(matches!(
            reader.fold_into(&mut failing),
            Err(FoldError::Sink {
                family: Family::Referents,
                ..
            })
        ));
        assert_eq!(
            failing
                .records
                .iter()
                .map(|record| record.record_identifier.payload().as_str())
                .collect::<Vec<_>>(),
            ["7z", "a1"]
        );
        assert_eq!(
            reader.enumerate().expect("read source after sink refusal"),
            before_inventory
        );
        assert_eq!(reader.database.list_tables(), before_catalog);
        assert_eq!(
            reader
                .database
                .current_database_marker()
                .expect("read source marker after refusal"),
            before_marker
        );

        let mut first = CollectingSink::default();
        let first_receipt = reader
            .fold_into(&mut first)
            .expect("first complete typed-sink fold");
        drop(reader);
        let mut second = CollectingSink::default();
        let second_receipt = LiveReader::open(&path)
            .expect("reopen frozen reader")
            .fold_into(&mut second)
            .expect("rerun complete typed-sink fold");
        assert_eq!(first_receipt, second_receipt);
        assert_eq!(first.inventory, second.inventory);
        let final_reader = LiveReader::open(&path).expect("reopen final source");
        assert_eq!(
            final_reader.enumerate().expect("read final source"),
            before_inventory
        );
        assert_eq!(final_reader.database.list_tables(), before_catalog);
        assert_eq!(
            final_reader
                .database
                .current_database_marker()
                .expect("read final source marker"),
            before_marker
        );
    }
}
