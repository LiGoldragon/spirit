//! `spirit-next` runtime.
//!
//! This crate is a running schema-derived Spirit pilot. The public wire
//! types are checked-in generated source from `schema/lib.schema` through
//! `schema-next` and `schema-rust-next`; the hand-written code here is the
//! runtime shim around those generated interfaces. `build.rs` verifies the
//! generated module is fresh.

#![forbid(unsafe_code)]

pub mod config;
pub mod daemon;
pub mod engine;
pub mod store;
pub mod transport;

pub mod schema {
    #[rustfmt::skip]
    pub mod lib;
}

pub use config::{Configuration, ConfigurationError};
pub use daemon::{Daemon, DaemonError};
pub use engine::{Engine, MailLedger, MailLedgerHook, SignalAccepted, SignalActor};
pub use schema::lib::{
    CommitSequence, DatabaseMarker, Description, Entry, ErrorMessage, ErrorReport, Export, Import,
    Input, InputNexus, InputRoute, Integer, Kind, LocalPath, Magnitude, MailIdentifier,
    MailLedgerEvent, MessageIdentifier, MessageProcessed, MessageProcessedHook, MessageRoot,
    MessageSent, MessageSentHook, NexusInput, NexusMail, NexusOutput, NexusReuse, ObservedRecords,
    Output, OutputRoute, ProcessedMail, PublicPath, Query, RecordIdentifier, RecordSet, SemaInput,
    SemaOutput, SemaReceipt, SemaReuse, SentMail, ShortHeader, SignalFrameError, SignalReuse,
    SourcePath, StateDigest, Topic,
};
pub use store::Store;
pub use transport::{SignalTransport, TransportError};
