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
pub mod meta_transport;
pub mod nexus;
mod plane;
#[cfg(feature = "production-migration")]
pub mod production_migration;
pub mod store;
pub mod subscription;
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
    #[rustfmt::skip]
    pub mod meta_signal;
    #[rustfmt::skip]
    pub mod daemon;
}

pub use config::ConfigurationError;
pub use daemon::{Daemon, SpiritDaemon, SpiritDaemonError};
pub use engine::{Engine, MailLedger, MailLedgerHook, SignalAccepted, SignalAdmission};
pub use meta_transport::{
    MetaFrameError, MetaInputRoute, MetaOutputRoute, MetaSignalTransport, MetaTransportError,
};
pub use nexus::{Nexus, StashTable};
#[cfg(feature = "production-migration")]
pub use production_migration::{
    ProductionMigration, ProductionMigrationCompleted, ProductionMigrationError,
    ProductionMigrationOutput, ProductionMigrationRequest,
};
pub use schema::daemon::{ComponentDaemon, DaemonCommand, DaemonEntry, DaemonError, ListenerTier};
pub use store::{Store, StoreError};
#[cfg(feature = "testing-trace")]
pub use trace::{TraceClient, TraceError, TraceLog, TraceSocketListener, TraceSocketPath};
pub use trace_event::{ObjectName, TraceEvent};
pub use transport::{SignalTransport, TransportError};
