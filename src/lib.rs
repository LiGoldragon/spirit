//! `spirit` runtime.
//!
//! This crate is a running schema-derived Spirit pilot. The public wire
//! types come from the generated `signal-spirit` contract, and owner-only meta
//! types come from the generated `meta-signal-spirit` contract. The daemon-local
//! Nexus, SEMA, and daemon modules are checked-in generated source through
//! `schema-next` and `schema-rust-next`. The hand-written code here is the
//! runtime shim around those generated interfaces. `build.rs` verifies the
//! generated modules are fresh.
//!
//! Plane envelopes make cross-plane mis-wiring a type error. A SEMA store
//! accepts only `sema::Sema<sema::WriteInput>` for durable writes and
//! `sema::Sema<sema::ReadInput>` for reads; a Nexus envelope with the same
//! inner payload names cannot be applied to the SEMA engine:
//!
//! ```compile_fail
//! use spirit::{
//!     Store,
//!     schema::{nexus::nexus as nexus_plane, sema::SemaEngine},
//! };
//!
//! let mut store: Store = todo!();
//! let message: nexus_plane::Nexus<nexus_plane::Work> = todo!();
//! let _ = store.apply(message);
//! ```

#![forbid(unsafe_code)]

pub mod config;
pub mod daemon;
pub mod engine;
#[cfg(feature = "agent-guardian")]
pub mod guardian;
#[cfg(feature = "agent-guardian")]
mod guardian_journal;
#[cfg(feature = "agent-guardian")]
mod guardian_prompt;
pub mod meta_transport;
pub mod nexus;
mod plane;
#[cfg(feature = "production-migration")]
pub mod production_migration;
#[cfg(feature = "nota-text")]
pub mod render;
pub mod store;
pub mod subscription;
#[cfg(feature = "testing-trace")]
pub mod trace;
pub mod trace_event;
pub mod transport;

pub mod schema {
    #[rustfmt::skip]
    pub mod domain {
        pub use signal_spirit::schema::domain::*;
    }
    #[rustfmt::skip]
    pub mod signal {
        pub use signal_spirit::schema::signal::*;
    }
    #[rustfmt::skip]
    pub mod nexus;
    #[rustfmt::skip]
    pub mod sema;
    #[rustfmt::skip]
    pub mod meta_signal {
        pub use meta_signal_spirit::schema::meta_signal::*;
    }
    #[rustfmt::skip]
    pub mod meta_signal_contract {
        pub use meta_signal_spirit::*;
    }
    #[rustfmt::skip]
    pub mod daemon;
}

pub use config::{Configuration, ConfigurationError};
pub use daemon::{Daemon, SpiritDaemon, SpiritDaemonError};
pub use engine::{
    Engine, MailIdentifier, MailLedger, MailLedgerEvent, MailLedgerHook, MessageIdentifier,
    MessageProcessed, MessageProcessedHook, MessageSent, MessageSentHook, OriginRoute,
    ProcessedMail, SentMail, ShortHeader, SignalAccepted, SignalAdmission, SignalObjectName,
    SignalResponse,
};
#[cfg(feature = "agent-guardian")]
pub use guardian::{
    AgentGuardian, AgentGuardianConfiguration, AgentGuardianError, AgentGuardianRejection,
};
pub use meta_transport::{
    MetaFrameError, MetaInputRoute, MetaOutputRoute, MetaSignalTransport, MetaTransportError,
};
pub use nexus::{Nexus, StashTable};
#[cfg(feature = "production-migration")]
pub use production_migration::{
    StoreMigration, StoreMigrationCompleted, StoreMigrationError, StoreMigrationOutput,
    StoreMigrationRequest,
};
pub use schema::daemon::{ComponentDaemon, DaemonCommand, DaemonEntry, DaemonError, ListenerTier};
pub use store::{Store, StoreError, StoreFamilyDirectory};
#[cfg(feature = "testing-trace")]
pub use trace::{TraceClient, TraceError, TraceLog, TraceSocketListener, TraceSocketPath};
pub use trace_event::{ObjectName, TraceEvent};
pub use transport::{SignalTransport, TransportError};
