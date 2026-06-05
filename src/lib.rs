//! `spirit` runtime.
//!
//! This crate is a running schema-derived Spirit pilot. The public wire
//! types are checked-in generated source from the three plane schemas
//! (`schema/signal.schema`, `schema/nexus.schema`, `schema/sema.schema`)
//! through `schema-next` and `schema-rust-next`; the hand-written code here is
//! the runtime shim around those generated interfaces. `build.rs` verifies the
//! generated modules are fresh.
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

extern crate self as spirit;

pub mod config;
pub mod daemon;
pub mod engine;
pub mod nexus;
mod plane;
pub mod store;
#[cfg(feature = "testing-trace")]
pub mod trace;
pub mod trace_event;
pub mod transport;

pub mod schema {
    #[rustfmt::skip]
    pub mod signal;
    #[rustfmt::skip]
    pub mod nexus;
    #[rustfmt::skip]
    pub mod sema;
}

pub use config::{Configuration, ConfigurationError};
pub use daemon::{Daemon, DaemonCommand, DaemonCommandError, DaemonError};
pub use engine::{Engine, MailLedger, MailLedgerHook, SignalAccepted, SignalActor};
pub use nexus::{Nexus, StashTable};
pub use schema::nexus::{
    ActorStartFailure as NexusActorStartFailure, ActorStopFailure as NexusActorStopFailure,
    CommandEffect, CommandSemaRead, CommandSemaWrite, Continue, EffectCompleted, NexusAction,
    NexusActionRoute, NexusEffectCommand, NexusEffectResult, NexusEngine, NexusObjectName,
    NexusRunnerNextStep, NexusWork, NexusWorkRoute, ReplyToSignal, SemaReadCompleted,
    SemaWriteCompleted, SignalArrived, Stash, StashRequest, StashResult, Stashed,
    nexus as nexus_plane,
};
pub use schema::sema::{
    ActorStartFailure as SemaActorStartFailure, ActorStopFailure as SemaActorStopFailure, Counted,
    Found, Missed, Observed, ReadInput as SemaReadInput, ReadInputRoute as SemaReadInputRoute,
    ReadOutput as SemaReadOutput, ReadOutputRoute as SemaReadOutputRoute, Recorded, Removed,
    SemaEngine, SemaObjectName, WriteInput as SemaWriteInput,
    WriteInputRoute as SemaWriteInputRoute, WriteOutput as SemaWriteOutput,
    WriteOutputRoute as SemaWriteOutputRoute, sema,
};
pub use schema::signal::{
    ActorStartFailure, ActorStopFailure, AtLeast, AtMost, Certainty, CertaintyChange,
    CertaintyChangeReceipt, CertaintyChanged, ChangeCertainty, CommitSequence, Count,
    CountedRecords, DatabaseMarker, Description, Entry, Error, ErrorMessage, ErrorReport, Exact,
    Export, FoundRecord, Full, Import, Input, InputRoute, Integer, Kind, LocalPath, Lookup,
    LookupStash, Magnitude, MailIdentifier, MailLedgerEvent, MessageIdentifier, MessageProcessed,
    MessageProcessedHook, MessageRoot, MessageSent, MessageSentHook, Observe, ObservedRecords,
    OriginRoute, Output, OutputRoute, Partial, Privacy, PrivacySelection, Processed, ProcessedMail,
    PublicPath, Query, Record, RecordAccepted, RecordCount, RecordFound, RecordIdentifier,
    RecordRemoved, RecordSet, Records, RecordsCounted, RecordsObserved, RecordsStashed, Rejected,
    Remove, RemoveReceipt, SemaReceipt, Sent, SentMail, ShortHeader, SignalEngine,
    SignalFrameError, SignalObjectName, SignalRejection, SignalReuse, SourcePath, StashHandle,
    StashedObservation, State, StateDigest, Statement, StatementText, Topic, TopicMatch, Topics,
    ValidationError, signal,
};
pub use store::{Store, StoreError};
#[cfg(feature = "testing-trace")]
pub use trace::{TraceClient, TraceError, TraceLog, TraceSocketListener, TraceSocketPath};
pub use trace_event::{ObjectName, TraceEvent};
pub use transport::{SignalTransport, TransportError};
