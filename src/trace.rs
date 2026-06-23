//! Spirit's actor-boundary trace sink.
//!
//! Every execution center (Signal admission, Nexus, the SEMA store) records its
//! boundary crossings into a shared [`TraceLog`]. The log keeps two
//! destinations at once (report 716):
//!
//! - an in-memory recording of spirit's own [`TraceEvent`]s, which spirit's
//!   tests read back verbatim via [`TraceLog::events`]; and
//! - an optional push socket that carries the SHARED wire contract
//!   [`signal_introspect::ComponentTraceEvent`], so the introspect daemon
//!   decodes the exact archived noun without ever depending on `spirit`.
//!
//! Call sites everywhere still hand the sink a `TraceEvent`; the projection onto
//! the contract type and the per-engine identity/sequence stamping happen here,
//! at the push boundary, so no actor-boundary emission point changes.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use signal_introspect::{ComponentTraceEvent, TraceSequence};
use signal_persona::EngineIdentifier;

use crate::TraceEvent;
pub use triad_runtime::trace::{TraceError, TraceEventFrame, TraceSocketPath};

pub type TraceClient = triad_runtime::trace::TraceClient<ComponentTraceEvent>;
pub type TraceSocketListener = triad_runtime::trace::TraceSocketListener<ComponentTraceEvent>;

/// The push target for the contract trace events, carried only when the daemon
/// is configured with a trace socket path.
#[derive(Clone, Debug)]
struct ComponentTraceSink {
    engine: EngineIdentifier,
    sequence: Arc<AtomicU64>,
    socket: triad_runtime::trace::TraceLog<ComponentTraceEvent>,
}

impl ComponentTraceSink {
    fn new(engine: EngineIdentifier, path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            engine,
            sequence: Arc::new(AtomicU64::new(0)),
            socket: triad_runtime::trace::TraceLog::socket(path),
        }
    }

    /// Project the spirit event onto the shared contract, stamping the emitting
    /// engine identity and the next monotonic per-emitter sequence, then push.
    fn push(&self, event: TraceEvent) {
        let mut projected = ComponentTraceEvent::from(event);
        projected.engine = self.engine.clone();
        projected.sequence = TraceSequence::new(self.sequence.fetch_add(1, Ordering::Relaxed));
        self.socket.record(projected);
    }
}

/// Spirit's trace sink: an in-memory recording of [`TraceEvent`] for spirit's
/// own tests, plus an optional contract-typed push socket for introspect.
#[derive(Clone, Debug)]
pub struct TraceLog {
    recording: triad_runtime::trace::TraceLog<TraceEvent>,
    component_sink: Option<ComponentTraceSink>,
}

impl Default for TraceLog {
    fn default() -> Self {
        Self::recording()
    }
}

impl TraceLog {
    /// In-memory only: record spirit's own events, push nothing.
    pub fn recording() -> Self {
        Self {
            recording: triad_runtime::trace::TraceLog::recording(),
            component_sink: None,
        }
    }

    /// In-memory recording PLUS a contract-typed push socket. The `engine`
    /// identity is stamped onto every pushed [`ComponentTraceEvent`].
    pub fn socket(engine: EngineIdentifier, path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            recording: triad_runtime::trace::TraceLog::recording(),
            component_sink: Some(ComponentTraceSink::new(engine, path)),
        }
    }

    /// Record one actor-boundary event: always into the in-memory recording,
    /// and, when a push socket is configured, projected onto the shared
    /// contract and pushed to introspect.
    pub fn record(&self, event: TraceEvent) {
        self.recording.record(event);
        if let Some(sink) = &self.component_sink {
            sink.push(event);
        }
    }

    /// The in-memory recording of spirit's own events, for spirit's tests.
    pub fn events(&self) -> Vec<TraceEvent> {
        self.recording.events()
    }
}

impl TraceEventFrame for TraceEvent {
    fn to_trace_archive(&self) -> Result<Vec<u8>, TraceError> {
        rkyv::to_bytes::<rkyv::rancor::Error>(self)
            .map(|archive| archive.to_vec())
            .map_err(|_| TraceError::ArchiveEncode)
    }

    fn from_trace_archive(archive: &[u8]) -> Result<Self, TraceError> {
        rkyv::from_bytes::<Self, rkyv::rancor::Error>(archive)
            .map_err(|_| TraceError::ArchiveDecode)
    }
}

#[cfg(feature = "nota-text")]
impl std::fmt::Display for TraceEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&<Self as nota::NotaEncode>::to_nota(self))
    }
}

#[cfg(feature = "nota-text")]
impl std::str::FromStr for TraceEvent {
    type Err = nota::NotaDecodeError;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        nota::NotaSource::new(source).parse::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use signal_introspect::{IntrospectionTarget, TraceLayer};

    use super::*;
    use crate::{ObjectName, SignalObjectName};

    #[test]
    fn projects_signal_admitted_onto_contract_event() {
        let event = TraceEvent::new(ObjectName::Signal(SignalObjectName::Admitted));
        let projected = ComponentTraceEvent::from(event);

        assert_eq!(projected.component, IntrospectionTarget::Signal);
        assert_eq!(projected.layer, TraceLayer::Signal);
        assert_eq!(projected.event_name, "SignalAdmitted");
    }
}
