//! Spirit's daemon hooks — the only daemon code spirit hand-writes.
//!
//! The uniform daemon skeleton (the `DaemonCommand` argv parsing, async task-backed
//! multi-listener binding, accepted-connection context, decode -> execute ->
//! encode spine, emitted subscription registry + retained-writer publish
//! wiring, and `ExitReport`-based entry) is emitted into
//! `src/schema/daemon.rs` by schema-rust-next's daemon emitter. Spirit fills
//! only the record-1488 escape hatches through `impl ComponentDaemon for
//! SpiritDaemon`: how to load its binary `Configuration`, how to open its
//! Store/Engine (`build_runtime`), how one working `Input` becomes one
//! `Output`, the owner-only meta request hook, and the stream filter + event
//! policy.

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use triad_runtime::{
    AcceptedConnection, EngineRequestError, FrameBody as LengthPrefixedFrameBody, FrameError,
    LengthPrefixedCodec, ListenerError,
};

use crate::{
    Configuration, ConfigurationError, Engine, StoreError,
    meta_transport::{MetaFrameError, MetaInput, MetaTransportError},
    schema::daemon::{ComponentDaemon, DaemonBinder, DaemonError},
    schema::signal::{
        EngineStartFailure, EngineStopFailure, Input, IntentEvent, Output, Query, SignalFrameError,
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

    #[error("daemon stream frame error: {0}")]
    StreamFrame(#[from] signal_frame::FrameError),

    #[error("daemon transport error: {0}")]
    Transport(#[from] TransportError),

    #[error("daemon meta transport error: {0}")]
    MetaTransport(#[from] MetaTransportError),

    #[error("daemon meta frame error: {0}")]
    MetaFrame(#[from] MetaFrameError),

    #[error("daemon sema store error: {0}")]
    Store(#[from] StoreError),

    #[error("daemon engine start error: {0}")]
    EngineStart(#[from] EngineStartFailure),

    #[error("daemon engine stop error: {0}")]
    EngineStop(#[from] EngineStopFailure),

    #[error("daemon engine request error: {0}")]
    EngineRequest(#[from] EngineRequestError),
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

    /// Open the engine and run its lifecycle start hooks. Engine startup needs
    /// exclusive `&mut` access (the SEMA → Nexus → Signal `on_start` chain), so
    /// it runs here at owned construction — before the engine is handed to the
    /// schema-emitted `EngineActor`, whose mailbox serialises every later
    /// request behind a shared `ActorRef`. The emitted `ComponentDaemon::start`
    /// / `stop` hooks take a shared `&Self::Engine` and stay the trait no-op
    /// default; the durable SEMA store releases on engine drop at shutdown.
    fn build_runtime(configuration: &Self::Configuration) -> Result<Self::Engine, Self::Error> {
        #[cfg(feature = "testing-trace")]
        let mut engine = {
            let trace_log = configuration
                .trace_socket_path()
                .map(TraceLog::socket)
                .unwrap_or_default();
            let store = Store::open_with_trace(configuration.database_path(), trace_log.clone())?;
            Engine::new_with_trace(store, trace_log)
        };
        #[cfg(not(feature = "testing-trace"))]
        let mut engine = {
            let store = Store::open(configuration.database_path())?;
            Engine::new(store)
        };
        #[cfg(feature = "agent-guardian")]
        if let Some(guardian) = configuration
            .guardian_agent_configuration()
            .cloned()
            .map(crate::guardian::AgentGuardian::new)
        {
            engine.set_guardian(guardian);
        } else {
            engine.require_guardian();
        }
        engine.start().map_err(Self::Error::from)?;
        Ok(engine)
    }

    async fn handle_working_input(
        engine: &mut Self::Engine,
        input: Input,
        _connection: &triad_runtime::ConnectionContext,
    ) -> Result<Output, Self::Error> {
        Ok(engine.handle_async(input).await.root().clone())
    }

    /// Serve one owner-only meta request: decode a `Configure` meta `Input`,
    /// apply it through `Engine::configure` (a configuration effect, not a SEMA
    /// log write), and write the `Configured` / `Rejected` meta `Output` back.
    /// `Configure` is request/reply, not a stream — no subscription handling.
    async fn handle_meta_connection(
        engine: &mut Self::Engine,
        mut connection: AcceptedConnection,
    ) -> Result<(), Self::Error> {
        let frame = LengthPrefixedCodec::default()
            .read_body_async(connection.stream_mut())
            .await?
            .into_bytes();
        let (_route, input) = MetaInput::decode_signal_frame(&frame)?;
        let MetaInput::Configure(request) = input;
        let reply = engine.configure_async(request.into_payload()).await;
        LengthPrefixedCodec::default()
            .write_body_async(
                connection.stream_mut(),
                &LengthPrefixedFrameBody::new(reply.encode_signal_frame()?),
            )
            .await?;
        connection
            .stream_mut()
            .flush()
            .await
            .map_err(FrameError::from)?;
        Ok(())
    }

    fn subscription_filter(input: &Input) -> Option<Self::SubscriptionFilter> {
        match input {
            Input::SubscribeIntent(query) => Some(query.payload().clone()),
            Input::State(_)
            | Input::Record(_)
            | Input::Propose(_)
            | Input::Clarify(_)
            | Input::Supersede(_)
            | Input::Retire(_)
            | Input::Observe(_)
            | Input::PublicRecords(_)
            | Input::PrivateRecords(_)
            | Input::Lookup(_)
            | Input::Count(_)
            | Input::Remove(_)
            | Input::ChangeCertainty(_)
            | Input::BumpImportance(_)
            | Input::ChangeRecord(_)
            | Input::RegisterReferent(_)
            | Input::LookupStash(_)
            | Input::CollectRemovalCandidates(_)
            | Input::Tap(_)
            | Input::Untap(_)
            | Input::Version => None,
        }
    }

    fn subscription_token(output: &Output) -> Option<Self::SubscriptionToken> {
        match output {
            Output::SubscriptionStarted(subscription) => {
                Some(IntentSubscriptionToken::from_signal_token(
                    subscription.payload().subscription_token.clone(),
                ))
            }
            _ => None,
        }
    }

    async fn published_event(
        engine: &Self::Engine,
        output: &Output,
    ) -> Result<Option<Self::StreamEvent>, Self::Error> {
        match output {
            Output::RecordAccepted(record_identifier) => Ok(engine
                .intent_recorded_event_async(record_identifier.payload())
                .await?),
            Output::Proposed(record_identifier) => Ok(engine
                .intent_recorded_event_async(record_identifier.payload())
                .await?),
            Output::Clarified(receipt) => Ok(engine
                .intent_clarified_event_async(receipt.payload())
                .await?),
            Output::Superseded(receipt) => Ok(engine
                .intent_superseded_event_async(receipt.payload())
                .await?),
            Output::Retired(receipt) => Ok(Some(engine.intent_retired_event(receipt.payload()))),
            _ => Ok(None),
        }
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
        tokio::runtime::Runtime::new()
            .map_err(DaemonError::Runtime)?
            .block_on(async {
                SpiritDaemon::bind(self.configuration)?
                    .run()
                    .await
                    .map_err(DaemonError::from)
            })
    }
}

impl Query {
    pub fn matches_intent_event(&self, event: &IntentEvent) -> bool {
        match event {
            IntentEvent::IntentRecorded(recorded) => recorded.entry.matches(self),
            IntentEvent::IntentClarified(clarified) => clarified.entry.matches(self),
            IntentEvent::IntentSuperseded(superseded) => superseded.entry.matches(self),
            IntentEvent::IntentRetired(_retired) => false,
        }
    }
}
