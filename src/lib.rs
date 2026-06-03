//! `spirit-next` runtime.
//!
//! This crate is a running schema-derived Spirit pilot. The public wire
//! types are checked-in generated source from `schema/lib.schema` through
//! `schema-next` and `schema-rust-next`; the hand-written code here is the
//! runtime shim around those generated interfaces. `build.rs` verifies the
//! generated module is fresh.
//!
//! Plane envelopes make cross-plane mis-wiring a type error. A SEMA store
//! accepts only `sema::Sema<sema::WriteInput>` for durable writes and
//! `sema::Sema<sema::ReadInput>` for reads; a Nexus envelope with the same
//! inner payload names cannot be applied to the SEMA engine:
//!
//! ```compile_fail
//! use spirit_next::{SemaEngine, Store, nexus_plane};
//!
//! let mut store: Store = todo!();
//! let message: nexus_plane::Nexus<nexus_plane::Work> = todo!();
//! let _ = store.apply(message);
//! ```

#![forbid(unsafe_code)]

pub mod config;
pub mod daemon;
pub mod engine;
pub mod nexus;
pub mod store;
#[cfg(feature = "testing-trace")]
pub mod trace;
pub mod transport;

pub mod schema {
    #[rustfmt::skip]
    pub mod lib;
}

pub use config::{Configuration, ConfigurationError};
pub use daemon::{Daemon, DaemonCommand, DaemonCommandError, DaemonError};
pub use engine::{Engine, MailLedger, MailLedgerHook, SignalAccepted, SignalActor};
pub use nexus::{ContinuationBudget, Nexus, StashTable};
pub use schema::lib::{
    ActorStartFailure, ActorStopFailure, CommitSequence, CountedRecords, DatabaseMarker,
    Description, Entry, ErrorMessage, ErrorReport, Export, FoundRecord, Import, Input, InputRoute,
    Integer, Kind, LocalPath, Magnitude, MailIdentifier, MailLedgerEvent, MessageIdentifier,
    MessageProcessed, MessageProcessedHook, MessageRoot, MessageSent, MessageSentHook, NexusAction,
    NexusActionRoute, NexusEffectCommand, NexusEffectResult, NexusEngine, NexusObjectName,
    NexusReuse, NexusWork, NexusWorkRoute, ObjectName, ObservedRecords, OriginRoute, Output,
    OutputRoute, Privacy, PrivacySelection, ProcessedMail, PublicPath, Query, RecordCount,
    RecordIdentifier, RecordSet, Records, RemoveReceipt, SemaEngine, SemaObjectName, SemaReadInput,
    SemaReadInputRoute, SemaReadOutput, SemaReadOutputRoute, SemaReceipt, SemaReuse,
    SemaWriteInput, SemaWriteInputRoute, SemaWriteOutput, SemaWriteOutputRoute, SentMail,
    ShortHeader, SignalEngine, SignalFrameError, SignalObjectName, SignalRejection, SignalReuse,
    SourcePath, StashHandle, StashRequest, StashResult, StashedObservation, StateDigest, Topic,
    TopicMatch, Topics, TraceEvent, ValidationError, nexus as nexus_plane, schema as schema_meta,
    sema, signal,
};
pub use store::{Store, StoreError};
#[cfg(feature = "testing-trace")]
pub use trace::{TraceClient, TraceError, TraceLog, TraceSocketListener, TraceSocketPath};
pub use transport::{SignalTransport, TransportError};
