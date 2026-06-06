//! Spirit's daemon hooks — the only daemon code spirit hand-writes.
//!
//! The uniform daemon skeleton (the `DaemonCommand` argv parsing, the decode ->
//! execute -> encode spine, the `{Single,Multi}ListenerDaemon` selection, the
//! option-B subscription registry + publish wiring, and the `ExitReport`-based
//! entry) is EMITTED into `src/schema/daemon.rs` by schema-rust-next's daemon
//! emitter, driven by the `NexusDaemonShape` in `build.rs`. Spirit fills only
//! the record-1488 escape hatches through `impl ComponentDaemon for
//! SpiritDaemon`: how to load its `Configuration`, how to open its Store/Engine
//! (`build_runtime`), how one working `Input` becomes one `Output`, the
//! owner-only meta escape hatch, and the stream filter + event policy.

use std::os::unix::net::UnixStream;

use thiserror::Error;
use triad_runtime::{FrameError, ListenerError};

use crate::{
    Configuration, ConfigurationError, Engine, StoreError,
    meta_transport::{MetaInput, MetaSignalTransport, MetaTransportError},
    schema::daemon::{ComponentDaemon, DaemonBinder, DaemonError},
    schema::signal::{
        ActorStartFailure, ActorStopFailure, Input, IntentEvent, Output, Query, SignalFrameError,
        short_header,
    },
    store::Store,
    subscription::IntentSubscriptionToken,
    transport::TransportError,
};

#[cfg(feature = "testing-trace")]
use crate::TraceLog;

/// The type-level selector for spirit's emitted daemon. It carries no runtime
/// data — it is the marker the emitted `DaemonCommand<SpiritDaemon>` and the
/// generated runtime dispatch on, selecting spirit's `Configuration` / `Engine`
/// / `Error` and the stream token/filter/event types through the
/// `ComponentDaemon` associated types.
#[derive(Debug)]
pub struct SpiritDaemon;

/// Spirit's daemon error: the engine-facing variants the emitted spine needs
/// (`From<FrameError>` / `From<SignalFrameError>` / `From<ListenerError>`) plus
/// spirit's domain errors. The emitted `DaemonError<SpiritDaemon>` wraps this
/// under its `Component` arm.
#[derive(Debug, Error)]
pub enum SpiritDaemonError {
    #[error("daemon IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("daemon frame error: {0}")]
    Frame(#[from] FrameError),

    #[error("daemon listener error: {0}")]
    Listener(#[from] ListenerError),

    #[error("daemon signal frame error: {0}")]
    SignalFrame(#[from] SignalFrameError),

    #[error("daemon transport error: {0}")]
    Transport(#[from] TransportError),

    #[error("daemon meta transport error: {0}")]
    MetaTransport(#[from] MetaTransportError),

    #[error("daemon sema store error: {0}")]
    Store(#[from] StoreError),

    #[error("daemon actor start error: {0}")]
    ActorStart(#[from] ActorStartFailure),

    #[error("daemon actor stop error: {0}")]
    ActorStop(#[from] ActorStopFailure),
}

impl ComponentDaemon for SpiritDaemon {
    type Configuration = Configuration;
    type ConfigurationError = ConfigurationError;
    type Engine = Engine;
    type Error = SpiritDaemonError;
    type SubscriptionToken = IntentSubscriptionToken;
    type SubscriptionFilter = Query;
    type StreamEvent = IntentEvent;

    const PROCESS_NAME: &'static str = "spirit-daemon";

    fn load_configuration(
        path: &std::path::Path,
    ) -> Result<Self::Configuration, Self::ConfigurationError> {
        Configuration::from_binary_path(path)
    }

    fn build_runtime(configuration: &Self::Configuration) -> Result<Self::Engine, Self::Error> {
        #[cfg(feature = "testing-trace")]
        {
            let trace_log = configuration
                .trace_socket_path()
                .map(TraceLog::socket)
                .unwrap_or_default();
            let store = Store::open_with_trace(configuration.database_path(), trace_log.clone())?;
            Ok(Engine::new_with_trace(store, trace_log))
        }
        #[cfg(not(feature = "testing-trace"))]
        {
            let store = Store::open(configuration.database_path())?;
            Ok(Engine::new(store))
        }
    }

    fn start(engine: &mut Self::Engine) -> Result<(), Self::Error> {
        engine.start().map_err(Self::Error::from)
    }

    fn stop(engine: &mut Self::Engine) -> Result<(), Self::Error> {
        engine.stop().map_err(Self::Error::from)
    }

    fn handle_working_input(
        engine: &Self::Engine,
        input: Input,
        _connection: &triad_runtime::ConnectionContext,
    ) -> Result<Output, Self::Error> {
        Ok(engine.handle(input).root().clone())
    }

    /// Serve one owner-only meta request: decode a `Configure` meta `Input`,
    /// apply it through `Engine::configure` (a configuration effect, not a SEMA
    /// log write), and write the `Configured` / `Rejected` meta `Output` back.
    /// `Configure` is request/reply, not a stream — no subscription handling.
    fn handle_meta_stream(engine: &Self::Engine, stream: UnixStream) -> Result<(), Self::Error> {
        let mut transport = MetaSignalTransport::new(stream);
        let (_route, input) = transport.read_input()?;
        let MetaInput::Configure(request) = input;
        let reply = engine.configure(request);
        transport.write_output(&reply)?;
        Ok(())
    }

    fn subscription_filter(input: &Input) -> Option<Self::SubscriptionFilter> {
        match input {
            Input::SubscribeIntent(query) => Some(query.clone()),
            Input::State(_)
            | Input::Record(_)
            | Input::Observe(_)
            | Input::Lookup(_)
            | Input::Count(_)
            | Input::Remove(_)
            | Input::ChangeCertainty(_)
            | Input::LookupStash(_)
            | Input::CollectRemovalCandidates(_)
            | Input::Tap(_)
            | Input::Untap(_) => None,
        }
    }

    fn subscription_token(output: &Output) -> Option<Self::SubscriptionToken> {
        match output {
            Output::SubscriptionStarted(subscription) => Some(
                IntentSubscriptionToken::from_signal_token(subscription.subscription_token),
            ),
            _ => None,
        }
    }

    fn published_event(
        engine: &Self::Engine,
        output: &Output,
    ) -> Result<Option<Self::StreamEvent>, Self::Error> {
        let Output::RecordAccepted(receipt) = output else {
            return Ok(None);
        };
        Ok(engine.intent_recorded_event(receipt)?)
    }

    fn event_matches_filter(filter: &Self::SubscriptionFilter, event: &Self::StreamEvent) -> bool {
        filter.matches_intent_event(event)
    }

    fn subscription_event_short_header() -> u64 {
        short_header::OUTPUT_EVENT
    }
}

/// A thin convenience wrapper so callers (tests, in-process launchers) keep the
/// familiar `Daemon::new(configuration).run()` surface over the emitted
/// `ComponentDaemon` binder. The bin uses the emitted `DaemonEntry` directly.
pub struct Daemon {
    configuration: Configuration,
}

impl Daemon {
    pub fn new(configuration: Configuration) -> Self {
        Self { configuration }
    }

    pub fn run(self) -> Result<(), DaemonError<SpiritDaemon>> {
        SpiritDaemon::bind(self.configuration)?
            .run()
            .map_err(DaemonError::from)
    }
}

impl Query {
    pub fn matches_intent_event(&self, event: &IntentEvent) -> bool {
        match event {
            IntentEvent::IntentRecorded(recorded) => recorded.entry.matches(self),
        }
    }
}
