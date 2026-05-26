//! `spirit-next` runtime.
//!
//! This crate is a running schema-derived Spirit pilot. The public wire
//! types are generated at build time from `schema/spirit.schema` through
//! `schema-next` and `schema-rust-next`; the hand-written code here is the
//! runtime shim around those generated interfaces.

#![forbid(unsafe_code)]

pub mod config;
pub mod daemon;
pub mod engine;
pub mod store;
pub mod transport;

pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/spirit_next_generated.rs"));
}

pub use config::{Configuration, ConfigurationError};
pub use daemon::{DaemonError, run_daemon};
pub use engine::Engine;
pub use generated::{
    Description, Entry, ErrorMessage, Input, InputRoute, Kind, Magnitude, Output, OutputRoute,
    Query, RecordIdentifier, RecordSet, SemaCommand, SemaCommandRoute, SemaResponse,
    SemaResponseRoute, SignalFrameError, Topic,
};
pub use store::Store;
pub use transport::{TransportError, exchange, read_input, read_output, write_input, write_output};
