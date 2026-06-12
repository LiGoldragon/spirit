//! Store migration as a logged fold.
//!
//! The previous store generations (schema versions 1 through 8) carry no
//! versioned operation log: spirit never opted into engine versioning before
//! schema version 9, and they predate the current engine's storage layout, so
//! the current engine refuses to open them at all. This module therefore
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
//! (`<stem>.schema-9-migrating-<pid>.sema`), then swaps it into place with
//! single-rename exposure: the previous store first gets a second name — a
//! hard link at the backup path (`<stem>.schema-old-backup-<N>.sema`, first
//! free `N`) — and ONE atomic rename then moves the fresh store over the
//! live path. The live path is therefore never absent. The previous store's
//! bytes survive every crash:
//!
//! - crash before the backup link: the live path still holds the previous
//!   store, untouched (the previous engine opens it read-only); at most a
//!   stale `.schema-9-migrating-*` temporary remains. Recovery: re-run
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
            Certainty, Description, Domain, Domains, Entry, Importance, Kind, Magnitude, Privacy,
            RecordIdentifier, Referent, Referents,
        },
    },
    store::ArchiveDatabase,
};

const SPIRIT_STORE_V1_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(1);
const SPIRIT_STORE_V2_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(2);
const SPIRIT_STORE_V3_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(3);
const SPIRIT_STORE_V4_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(4);
const SPIRIT_STORE_V5_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(5);
const SPIRIT_STORE_V6_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(6);
const SPIRIT_STORE_V7_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(7);
const SPIRIT_STORE_V8_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(8);
const SPIRIT_STORE_CURRENT_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(9);
const RECORDS_TABLE: TableName = TableName::new("records");
const REFERENTS_TABLE: TableName = TableName::new("referents");

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
    Previous(SchemaVersion),
}

struct SpiritStoreV1Database {
    database: PreviousSemaDatabase,
    records: PreviousTableReference<SpiritStoreV1Record>,
}

struct SpiritStoreV2Database {
    database: PreviousSemaDatabase,
    records: PreviousTableReference<SpiritStoreV2Record>,
}

struct SpiritStoreV4Database {
    database: PreviousSemaDatabase,
    records: PreviousTableReference<SpiritStoreV4Record>,
}

struct SpiritStoreV5Database {
    database: PreviousSemaDatabase,
    records: PreviousTableReference<SpiritStoreV5Record>,
}

struct SpiritStoreV6Database {
    database: PreviousSemaDatabase,
    records: PreviousTableReference<SpiritStoreV6Record>,
}

struct SpiritStoreV7Database {
    database: PreviousSemaDatabase,
    records: PreviousTableReference<SpiritStoreV7Record>,
}

struct SpiritStoreV8Database {
    database: PreviousSemaDatabase,
    records: PreviousTableReference<SpiritStoreV8Record>,
    referents: Option<PreviousTableReference<SpiritStoreV8Referent>>,
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

// The version-8 stored shapes: the last pre-versioning generation. The entry
// is already the current generated `Entry`; only the storage registration
// (hand-typed `String` identifier, no family identity, no versioned log)
// distinguishes v8 rows from the v9 store's schema-declared `StoredRecord`.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV8Record {
    record_identifier: String,
    entry: Entry,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
struct SpiritStoreV8Referent {
    referent: Referent,
    aliases: Referents,
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
                Self::Programming => {
                    String::from("(Technology (Software (Languages ProgrammingLanguages)))")
                }
                Self::Architecture => {
                    String::from("(Technology (Software (Engineering SoftwareArchitecture)))")
                }
                Self::Schema => String::from("(Technology (Software (Data SchemaEvolution)))"),
                Self::Infrastructure => {
                    String::from("(Technology (Software (Operations InfrastructureAsCode)))")
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
            SourceStoreVersion::Current => {
                let store = Store::open(&database_path)?;
                Ok(StoreMigrationOutput::current(StoreMigrationCompleted {
                    record_count: store.len() as u64,
                    referent_count: 0,
                }))
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
        match PreviousSemaDatabase::open(PreviousEngineOpen::new(
            database_path,
            SPIRIT_STORE_V8_SCHEMA_VERSION,
        )) {
            Ok(_) => Ok(SourceStoreVersion::Previous(SPIRIT_STORE_V8_SCHEMA_VERSION)),
            Err(sema_engine_previous::Error::Sema(StorageKernelError::SchemaVersionMismatch {
                found,
                ..
            })) => {
                if found == SPIRIT_STORE_CURRENT_SCHEMA_VERSION {
                    Ok(SourceStoreVersion::Current)
                } else if found >= SPIRIT_STORE_V1_SCHEMA_VERSION
                    && found <= SPIRIT_STORE_V7_SCHEMA_VERSION
                {
                    Ok(SourceStoreVersion::Previous(found))
                } else {
                    Err(StoreMigrationError::UnknownSchemaVersion { found })
                }
            }
            Err(error) => Err(error.into()),
        }
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
            SPIRIT_STORE_V1_SCHEMA_VERSION => {
                SpiritStorePreviousStore::from_v1(SpiritStoreV1Database::open(&database_path)?)
            }
            SPIRIT_STORE_V2_SCHEMA_VERSION | SPIRIT_STORE_V3_SCHEMA_VERSION => {
                SpiritStorePreviousStore::from_v2(SpiritStoreV2Database::open(
                    &database_path,
                    previous_schema_version,
                )?)
            }
            SPIRIT_STORE_V4_SCHEMA_VERSION => {
                SpiritStorePreviousStore::from_v4(SpiritStoreV4Database::open(&database_path)?)
            }
            SPIRIT_STORE_V5_SCHEMA_VERSION => {
                SpiritStorePreviousStore::from_v5(SpiritStoreV5Database::open(&database_path)?)
            }
            SPIRIT_STORE_V6_SCHEMA_VERSION => {
                SpiritStorePreviousStore::from_v6(SpiritStoreV6Database::open(&database_path)?)
            }
            SPIRIT_STORE_V7_SCHEMA_VERSION => {
                SpiritStorePreviousStore::from_v7(SpiritStoreV7Database::open(&database_path)?)
            }
            SPIRIT_STORE_V8_SCHEMA_VERSION => {
                SpiritStorePreviousStore::from_v8(SpiritStoreV8Database::open_live(&database_path)?)
            }
            _ => unreachable!("migration is only called for known previous schema versions"),
        }?;
        let temporary_path = Self::temporary_path(&database_path);
        if temporary_path.exists() {
            fs::remove_file(&temporary_path)?;
        }
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
                previous_schema_version.value(),
            )),
            migrated_record_count: MigratedRecordCount::new(record_count),
            migrated_referent_count: MigratedReferentCount::new(referent_count),
        })?;
        drop(target_store);
        self.migrate_archive_sibling(&database_path, previous_schema_version)?;
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
        let archive_records =
            SpiritStoreV8Database::open_archive(&archive_path)?.archived_records()?;
        let temporary_path = Self::temporary_path(&archive_path);
        if temporary_path.exists() {
            fs::remove_file(&temporary_path)?;
        }
        let mut fresh_archive = ArchiveDatabase::open(&temporary_path)?;
        for record in archive_records {
            fresh_archive.import_archived_record(StoredRecord {
                record_identifier: RecordIdentifier::new(record.record_identifier),
                entry: record.entry,
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
        database_path.with_extension(format!("schema-9-migrating-{}.sema", std::process::id()))
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

impl SpiritStoreV1Database {
    fn open(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            path,
            SPIRIT_STORE_V1_SCHEMA_VERSION,
        ))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV1Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV2Database {
    fn open(path: &Path, schema_version: SchemaVersion) -> Result<Self, StoreMigrationError> {
        let mut database =
            PreviousSemaDatabase::open(PreviousEngineOpen::new(path, schema_version))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV2Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV4Database {
    fn open(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            path,
            SPIRIT_STORE_V4_SCHEMA_VERSION,
        ))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV4Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV5Database {
    fn open(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            path,
            SPIRIT_STORE_V5_SCHEMA_VERSION,
        ))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV5Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV6Database {
    fn open(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            path,
            SPIRIT_STORE_V6_SCHEMA_VERSION,
        ))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV6Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV7Database {
    fn open(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            path,
            SPIRIT_STORE_V7_SCHEMA_VERSION,
        ))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self { database, records })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV7Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }
}

impl SpiritStoreV8Database {
    /// Open a version-8 LIVE store: records plus the referent registry.
    fn open_live(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            path,
            SPIRIT_STORE_V8_SCHEMA_VERSION,
        ))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        let referents = database.register_table(PreviousTableDescriptor::new(REFERENTS_TABLE))?;
        Ok(Self {
            database,
            records,
            referents: Some(referents),
        })
    }

    /// Open a version-8 ARCHIVE store: the archive only ever registered the
    /// records table, and registering the referents table here would write a
    /// catalog row into the source being migrated.
    fn open_archive(path: &Path) -> Result<Self, StoreMigrationError> {
        let mut database = PreviousSemaDatabase::open(PreviousEngineOpen::new(
            path,
            SPIRIT_STORE_V8_SCHEMA_VERSION,
        ))?;
        let records = database.register_table(PreviousTableDescriptor::new(RECORDS_TABLE))?;
        Ok(Self {
            database,
            records,
            referents: None,
        })
    }

    fn records(&self) -> Result<Vec<SpiritStoreV8Record>, StoreMigrationError> {
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(self.records))?
            .records()
            .to_vec())
    }

    fn archived_records(&self) -> Result<Vec<SpiritStoreV8Record>, StoreMigrationError> {
        self.records()
    }

    fn referents(&self) -> Result<Vec<SpiritStoreV8Referent>, StoreMigrationError> {
        let Some(referents) = self.referents else {
            return Ok(Vec::new());
        };
        Ok(self
            .database
            .match_records(PreviousQueryPlan::all(referents))?
            .records()
            .to_vec())
    }
}

/// The previous store's rows after conversion through the historical
/// `From`-chain: current-shape records plus (from version 8 on) the
/// registered referents, ready for the logged fold into a fresh store.
struct SpiritStorePreviousStore {
    records: Vec<SpiritStorePreviousRecord>,
    referents: Vec<StoredReferent>,
}

struct SpiritStorePreviousRecord {
    record_identifier: String,
    entry: Entry,
}

impl SpiritStorePreviousStore {
    fn from_v1(database: SpiritStoreV1Database) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v1)
                .collect(),
            referents: Vec::new(),
        })
    }

    fn from_v2(database: SpiritStoreV2Database) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v2)
                .collect(),
            referents: Vec::new(),
        })
    }

    fn from_v4(database: SpiritStoreV4Database) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v4)
                .collect(),
            referents: Vec::new(),
        })
    }

    fn from_v5(database: SpiritStoreV5Database) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v5)
                .collect(),
            referents: Vec::new(),
        })
    }

    fn from_v6(database: SpiritStoreV6Database) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v6)
                .collect(),
            referents: Vec::new(),
        })
    }

    fn from_v7(database: SpiritStoreV7Database) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v7)
                .collect::<Result<Vec<_>, _>>()?,
            referents: Vec::new(),
        })
    }

    fn from_v8(database: SpiritStoreV8Database) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            records: database
                .records()?
                .into_iter()
                .map(SpiritStorePreviousRecord::from_v8)
                .collect(),
            referents: database
                .referents()?
                .into_iter()
                .map(StoredReferent::from)
                .collect(),
        })
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

    fn from_v7(record: SpiritStoreV7Record) -> Result<Self, StoreMigrationError> {
        Ok(Self {
            record_identifier: record.record_identifier,
            entry: record.entry.into_new_entry()?,
        })
    }

    fn from_v8(record: SpiritStoreV8Record) -> Self {
        Self {
            record_identifier: record.record_identifier,
            entry: record.entry,
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

impl PreviousEngineRecord for SpiritStoreV1Record {
    fn record_key(&self) -> PreviousRecordKey {
        PreviousRecordKey::new(self.record_identifier.clone())
    }
}

impl PreviousEngineRecord for SpiritStoreV2Record {
    fn record_key(&self) -> PreviousRecordKey {
        PreviousRecordKey::new(self.record_identifier.clone())
    }
}

impl PreviousEngineRecord for SpiritStoreV4Record {
    fn record_key(&self) -> PreviousRecordKey {
        PreviousRecordKey::new(self.record_identifier.clone())
    }
}

impl PreviousEngineRecord for SpiritStoreV5Record {
    fn record_key(&self) -> PreviousRecordKey {
        PreviousRecordKey::new(self.record_identifier.clone())
    }
}

impl PreviousEngineRecord for SpiritStoreV6Record {
    fn record_key(&self) -> PreviousRecordKey {
        PreviousRecordKey::new(self.record_identifier.clone())
    }
}

impl PreviousEngineRecord for SpiritStoreV7Record {
    fn record_key(&self) -> PreviousRecordKey {
        PreviousRecordKey::new(self.record_identifier.clone())
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
    use sema_engine_previous::{
        Assertion as PreviousAssertion, Engine as PreviousSemaDatabase,
        EngineOpen as PreviousEngineOpen, TableDescriptor as PreviousTableDescriptor,
    };

    use super::{
        RECORDS_TABLE, REFERENTS_TABLE, SPIRIT_STORE_V7_SCHEMA_VERSION,
        SPIRIT_STORE_V8_SCHEMA_VERSION, SpiritStoreV7Entry, SpiritStoreV7Record,
        SpiritStoreV8Record, SpiritStoreV8Referent, StoreMigration, StoreMigrationOutput,
        StoreMigrationRequest, store_version_seven,
    };
    use crate::{
        Store,
        schema::{
            sema::RecordFamily,
            signal::{
                Certainty, Data, Description, Domain, Domains, Entry, Importance, Information,
                Kind, Magnitude, Operations, Privacy, Referent, Referents, Software, Technology,
            },
        },
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
                entry: version_eight_entry(
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
                entry: version_eight_entry("migration is a logged fold", Vec::new()),
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
                    entry: version_eight_entry("archived record survives", Vec::new()),
                },
            ))
            .expect("seed version eight archived record");
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
                Domain::Technology(Technology::Software(Software::Data(Data::SchemaEvolution))),
                Domain::Technology(Technology::Software(Software::Operations(
                    Operations::InfrastructureAsCode
                ))),
                Domain::Information(Information::Documentation),
            ])
        );
        let migrations = target_store.migrations().expect("read migration markers");
        assert_eq!(migrations.len(), 1);
        assert_eq!(*migrations[0].source_schema_version.payload(), 7);
    }
}
