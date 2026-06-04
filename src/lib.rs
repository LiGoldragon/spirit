//! `spirit` runtime.
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
//! use spirit::{SemaEngine, Store, nexus_plane};
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
pub use nexus::{Nexus, StashTable};
pub use schema::lib::{
    ActorStartFailure, ActorStopFailure, AtLeast, AtMost, CommandEffect, CommandSemaRead,
    CommandSemaWrite, CommitSequence, Continue, Count, Counted, CountedRecords, DatabaseMarker,
    Description, EffectCompleted, Entry, Error, ErrorMessage, ErrorReport, Exact, Export, Found,
    FoundRecord, Full, Import, Input, InputRoute, Integer, Kind, LocalPath, Lookup, LookupStash,
    Magnitude, MailIdentifier, MailLedgerEvent, MessageIdentifier, MessageProcessed,
    MessageProcessedHook, MessageRoot, MessageSent, MessageSentHook, Missed, NexusAction,
    NexusActionRoute, NexusEffectCommand, NexusEffectResult, NexusEngine, NexusObjectName,
    NexusReuse, NexusWork, NexusWorkRoute, ObjectName, Observe, Observed, ObservedRecords,
    OriginRoute, Output, OutputRoute, Partial, Privacy, PrivacySelection, Processed, ProcessedMail,
    PublicPath, Query, Record, RecordAccepted, RecordCount, RecordFound, RecordIdentifier,
    RecordRemoved, RecordSet, Recorded, Records, RecordsCounted, RecordsObserved, RecordsStashed,
    Rejected, Remove, RemoveReceipt, Removed, ReplyToSignal, SemaEngine, SemaObjectName,
    SemaReadCompleted, SemaReadInput, SemaReadInputRoute, SemaReadOutput, SemaReadOutputRoute,
    SemaReceipt, SemaReuse, SemaWriteCompleted, SemaWriteInput, SemaWriteInputRoute,
    SemaWriteOutput, SemaWriteOutputRoute, Sent, SentMail, ShortHeader, SignalArrived,
    SignalEngine, SignalFrameError, SignalObjectName, SignalRejection, SignalReuse, SourcePath,
    Stash, StashHandle, StashRequest, StashResult, Stashed, StashedObservation, StateDigest, Topic,
    TopicMatch, Topics, TraceEvent, ValidationError, nexus as nexus_plane, schema as schema_meta,
    sema, signal,
};
pub use store::{Store, StoreError};
#[cfg(feature = "testing-trace")]
pub use trace::{TraceClient, TraceError, TraceLog, TraceSocketListener, TraceSocketPath};
pub use transport::{SignalTransport, TransportError};
