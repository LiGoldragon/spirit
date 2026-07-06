//! Spirit's daemon hooks — the only daemon code spirit hand-writes.
//!
//! The uniform daemon skeleton (the `DaemonCommand` argv parsing, async task-backed
//! multi-listener binding, accepted-connection context, decode -> execute ->
//! encode spine, emitted subscription registry + retained-writer publish
//! wiring, and `ExitReport`-based entry) is emitted into
//! `src/schema/daemon.rs` by schema-rust's daemon emitter. Spirit fills
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
    schema::nexus::{EngineStartFailure, EngineStopFailure},
    schema::signal::{Input, IntentEvent, Output, Query, SignalFrameError, short_header},
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
            // The pushed `ComponentTraceEvent`s are stamped with this engine's
            // identity so introspect can key its store per emitter. The daemon
            // socket path uniquely identifies this running spirit instance.
            let engine_identity = signal_persona::EngineIdentifier::new(
                configuration.socket_path().to_string_lossy().into_owned(),
            );
            let trace_log = configuration
                .trace_socket_path()
                .map(|path| TraceLog::socket(engine_identity.clone(), path))
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
        engine.set_authorization_mode(configuration.authorization_mode());
        engine.start().map_err(Self::Error::from)?;
        Ok(engine)
    }

    async fn handle_working_input(
        engine: &mut Self::Engine,
        input: Input,
        _connection: &triad_runtime::ConnectionContext,
    ) -> Result<Output, Self::Error> {
        let output = engine.handle_async(input).await.root().clone();
        // THE CRIOME AUTHORIZE-AND-SHIP SEAM (Spirit `xhwa`), DORMANT.
        //
        // The spirit-side `CriomeAuthorization` policy decides everything
        // here. `Disabled` — the operative default until criome-cluster
        // authorization is ready — keeps spirit fully local: the working
        // write above advanced the head freely, and `gate_and_ship_head`
        // returns immediately with no head capture, no authorization request,
        // and no mirror ship. `Enabled` already refused any head-advancing
        // input inside `handle_async` (fail-closed, no cluster authorizer
        // exists yet); for a pre-existing head the dormant `CriomeGate` seam
        // answers `Unconfigured`, which never ships.
        //
        // Present only under the `mirror-shipper` feature. A seam-machinery
        // fault never fails the working reply: the local commit (when the
        // policy admitted it) already landed durably.
        #[cfg(feature = "mirror-shipper")]
        match engine.gate_and_ship_head().await {
            Ok(_decision) => {}
            Err(error) => {
                let _ = error;
            }
        }
        Ok(output)
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
        let reply = match input {
            MetaInput::Configure(request) => engine.configure_async(request.into_payload()).await,
            MetaInput::Import(request) => engine.import_async(request.into_payload()).await,
            MetaInput::CollectRemovalCandidates(request) => {
                engine
                    .collect_removal_candidates_async(request.into_payload())
                    .await
            }
            MetaInput::ObserveHead => engine.observe_head_async().await,
            MetaInput::ObserveHeadObject => engine.observe_head_object_async().await,
        };
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
            | Input::ResolveClarification(_)
            | Input::Supersede(_)
            | Input::Retire(_)
            | Input::Observe(_)
            | Input::PublicIntent(_)
            | Input::PublicTextSearch(_)
            | Input::PublicRecords(_)
            | Input::PrivateRecords(_)
            | Input::Lookup(_)
            | Input::Count(_)
            | Input::BumpImportance(_)
            | Input::ChangeRecord(_)
            | Input::RegisterReferent(_)
            | Input::LookupStash(_)
            | Input::Tap(_)
            | Input::Untap(_)
            | Input::Version
            | Input::ApplyAuthorizedRecord(_)
            | Input::Marker => None,
        }
    }

    fn subscription_token(output: &Output) -> Option<Self::SubscriptionToken> {
        match output {
            Output::SubscriptionStarted(subscription) => {
                Some(IntentSubscriptionToken::from_signal_token(
                    subscription.payload().payload().clone(),
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
