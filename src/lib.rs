//! `spirit-next` runtime.
//!
//! This crate is a running schema-derived Spirit pilot. The public wire
//! types are checked-in generated source from `schema/lib.schema` through
//! `schema-next` and `schema-rust-next`; the hand-written code here is the
//! runtime shim around those generated interfaces. `build.rs` verifies the
//! generated module is fresh.
//!
//! Plane envelopes make cross-plane mis-wiring a type error. A SEMA store
//! accepts only a `sema::Sema<sema::Input>` root object; a Nexus envelope
//! with the same inner payload names cannot be applied to the SEMA engine:
//!
//! ```compile_fail
//! use spirit_next::{SemaEngine, Store, nexus_plane};
//!
//! let mut store: Store = todo!();
//! let message: nexus_plane::Nexus<nexus_plane::Input> = todo!();
//! let _ = store.apply(message);
//! ```

#![forbid(unsafe_code)]

pub mod config;
pub mod daemon;
pub mod engine;
pub mod nexus;
pub mod store;
pub mod transport;

pub mod schema {
    #[rustfmt::skip]
    pub mod lib;
}

pub use config::{Configuration, ConfigurationError};
pub use daemon::{Daemon, DaemonError};
pub use engine::{Engine, MailLedger, MailLedgerHook, SignalAccepted, SignalActor};
pub use nexus::{BeingProcessed, FromMail, Mail, Nexus, Processed};
pub use schema::lib::{
    CommitSequence, DatabaseMarker, Description, Entry, ErrorMessage, ErrorReport, Export, Import,
    Input, InputNexus, InputRoute, Integer, Kind, LocalPath, Magnitude, MailIdentifier,
    MailLedgerEvent, MessageIdentifier, MessageProcessed, MessageProcessedHook, MessageRoot,
    MessageSent, MessageSentHook, NexusEngine, NexusInput, NexusMail, NexusOutput, NexusReuse,
    ObservedRecords, OriginRoute, Output, OutputRoute, ProcessedMail, PublicPath, Query,
    RecordIdentifier, RecordSet, SemaEngine, SemaInput, SemaOutput, SemaReceipt, SemaReuse,
    SentMail, ShortHeader, SignalFrameError, SignalRejection, SignalReuse, SourcePath, StateDigest,
    Topic, ValidationError, nexus as nexus_plane, schema as schema_meta, sema, signal,
};
pub use store::{Store, StoreError};
pub use transport::{SignalTransport, TransportError};
