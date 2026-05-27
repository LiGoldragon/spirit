//! `spirit-next` runtime.
//!
//! This crate is a running schema-derived Spirit pilot. The public wire
//! types are generated at build time from `schema/lib.schema` through
//! `schema-next` and `schema-rust-next`; the hand-written code here is the
//! runtime shim around those generated interfaces.

#![forbid(unsafe_code)]

pub mod config;
pub mod daemon;
pub mod engine;
pub mod store;
pub mod transport;

pub mod schema {
    pub mod lib {
        include!(concat!(env!("OUT_DIR"), "/schema/lib.rs"));
    }
}

pub use config::{Configuration, ConfigurationError};
pub use daemon::{Daemon, DaemonError};
pub use engine::Engine;
pub use schema::lib::{
    Description, Entry, ErrorMessage, Input, InputRoute, Kind, Magnitude, Output, OutputRoute,
    Query, RecordIdentifier, RecordSet, SemaCommand, SemaResponse, SignalFrameError, Topic,
};
pub use store::Store;
pub use transport::{SignalTransport, TransportError};
