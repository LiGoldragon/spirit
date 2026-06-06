use std::{
    fmt::{Display, Formatter},
    os::unix::net::UnixStream,
};

use thiserror::Error;
use triad_runtime::{
    ArgumentError, ComponentArgument, ComponentCommand, ListenerError, ListenerSocket,
    MultiListenerDaemon, MultiListenerDaemonError, MultiListenerRuntime, RequestErrorLog, SocketMode,
};

use crate::{
    Configuration, ConfigurationError, Engine, StoreError,
    meta_transport::{MetaInput, MetaSignalTransport, MetaTransportError},
    schema::signal::{ActorStartFailure, ActorStopFailure, Input, Output, Query},
    store::Store,
    subscription::{SubscriptionError, SubscriptionHub},
    transport::{SignalTransport, TransportError},
};

/// The owner-only meta socket file mode: readable and writable by the owner
/// uid only (`rw-------`). triad-runtime has no peer-credential check, so the
/// meta surface's owner-only authority rests entirely on this filesystem mode.
const OWNER_ONLY_SOCKET_MODE: u32 = 0o600;

/// Which authority-tiered socket an arriving stream belongs to. `Working` is
/// the peer-callable signal surface (`schema/signal.schema`); `Meta` is the
/// owner-only `Configure` policy surface (`schema/meta-signal.schema`). Used as
/// the `MultiListenerRuntime::Listener` tag so `handle_stream` decodes the
/// correct wire contract for the arriving socket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpiritListener {
    Working,
    Meta,
}

impl Display for SpiritListener {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Working => formatter.write_str("working"),
            Self::Meta => formatter.write_str("meta"),
        }
    }
}

#[cfg(feature = "testing-trace")]
use crate::TraceLog;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("daemon IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("daemon listener error: {0}")]
    Listener(#[from] ListenerError),

    #[error("daemon transport error: {0}")]
    Transport(#[from] TransportError),

    #[error("daemon meta transport error: {0}")]
    MetaTransport(#[from] MetaTransportError),

    #[error("daemon subscription error: {0}")]
    Subscription(#[from] SubscriptionError),

    #[error("daemon sema store error: {0}")]
    Store(#[from] StoreError),

    #[error("daemon actor start error: {0}")]
    ActorStart(#[from] ActorStartFailure),

    #[error("daemon actor stop error: {0}")]
    ActorStop(#[from] ActorStopFailure),
}

#[derive(Debug, Error)]
pub enum DaemonCommandError {
    #[error("daemon argument error: {0}")]
    Argument(#[from] ArgumentError),

    #[error("{0}")]
    Configuration(#[from] ConfigurationError),

    #[error("{0}")]
    Daemon(#[from] DaemonError),
}

pub struct DaemonCommand {
    command: ComponentCommand,
}

impl DaemonCommand {
    pub fn from_environment() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
        }
    }

    pub fn from_arguments<Arguments, Argument>(arguments: Arguments) -> Self
    where
        Arguments: IntoIterator<Item = Argument>,
        Argument: Into<String>,
    {
        Self {
            command: ComponentCommand::from_arguments(arguments),
        }
    }

    pub fn configuration(&self) -> Result<Configuration, DaemonCommandError> {
        match self.command.signal_file_argument()? {
            ComponentArgument::SignalFile(file) => {
                Configuration::from_binary_path(file.as_path()).map_err(Into::into)
            }
            ComponentArgument::InlineNota(_) | ComponentArgument::NotaFile(_) => {
                Err(ArgumentError::ExpectedSignalFile.into())
            }
        }
    }

    pub fn run(&self) -> Result<(), DaemonCommandError> {
        Daemon::new(self.configuration()?).run().map_err(Into::into)
    }
}

pub struct Daemon {
    configuration: Configuration,
}

impl Daemon {
    pub fn new(configuration: Configuration) -> Self {
        Self { configuration }
    }

    pub fn run(&self) -> Result<(), DaemonError> {
        let runtime = self.runtime()?;
        let mut sockets = vec![ListenerSocket::new(
            SpiritListener::Working,
            self.configuration.socket_path().to_path_buf(),
        )];
        if let Some(meta_socket_path) = self.configuration.meta_socket_path() {
            sockets.push(
                ListenerSocket::new(SpiritListener::Meta, meta_socket_path.to_path_buf())
                    .with_socket_mode(SocketMode::new(OWNER_ONLY_SOCKET_MODE)),
            );
        }
        MultiListenerDaemon::new(sockets, runtime, RequestErrorLog::new("spirit-daemon"))
            .run()
            .map_err(Into::into)
    }

    fn runtime(&self) -> Result<SpiritDaemonRuntime, DaemonError> {
        #[cfg(feature = "testing-trace")]
        {
            let trace_log = self
                .configuration
                .trace_socket_path()
                .map(TraceLog::socket)
                .unwrap_or_default();
            let store =
                Store::open_with_trace(self.configuration.database_path(), trace_log.clone())?;
            Ok(SpiritDaemonRuntime::new(Engine::new_with_trace(
                store, trace_log,
            )))
        }
        #[cfg(not(feature = "testing-trace"))]
        {
            let store = Store::open(self.configuration.database_path())?;
            Ok(SpiritDaemonRuntime::new(Engine::new(store)))
        }
    }
}

struct SpiritDaemonRuntime {
    engine: Engine,
    subscriptions: SubscriptionHub,
}

impl SpiritDaemonRuntime {
    fn new(engine: Engine) -> Self {
        Self {
            engine,
            subscriptions: SubscriptionHub::default(),
        }
    }

    fn handle_working_stream(&self, stream: UnixStream) -> Result<(), DaemonError> {
        let subscription_writer = stream.try_clone()?;
        let mut transport = SignalTransport::new(stream);
        let (_route, input) = transport.read_input()?;
        let subscription_filter = SubscriptionFilter::from_input(&input);
        let output = self.engine.handle(input);
        let root_output = output.root().clone();
        transport.write_output(&root_output)?;
        self.register_subscription(subscription_filter, &root_output, subscription_writer);
        self.publish_output(&root_output)?;
        Ok(())
    }

    /// Serve one owner-only meta request: decode a `Configure` meta `Input`,
    /// apply it through `Engine::configure` (a configuration effect, not a SEMA
    /// log write), and write the `Configured` / `Rejected` meta `Output` back.
    /// No subscription registration — `Configure` is request/reply, not a
    /// stream.
    fn handle_meta_stream(&self, stream: UnixStream) -> Result<(), DaemonError> {
        let mut transport = MetaSignalTransport::new(stream);
        let (_route, input) = transport.read_input()?;
        let MetaInput::Configure(request) = input;
        let reply = self.engine.configure(request);
        transport.write_output(&reply)?;
        Ok(())
    }

    fn register_subscription(
        &self,
        filter: Option<SubscriptionFilter>,
        output: &Output,
        writer: UnixStream,
    ) {
        let Some(filter) = filter else {
            return;
        };
        let Output::SubscriptionStarted(subscription) = output else {
            return;
        };
        self.subscriptions
            .register(subscription.subscription_token, filter.into_query(), writer);
    }

    fn publish_output(&self, output: &Output) -> Result<(), DaemonError> {
        let Output::RecordAccepted(receipt) = output else {
            return Ok(());
        };
        if let Some(event) = self.engine.intent_recorded_event(receipt)? {
            self.subscriptions.publish(event)?;
        }
        Ok(())
    }
}

struct SubscriptionFilter {
    query: Query,
}

impl SubscriptionFilter {
    fn from_input(input: &Input) -> Option<Self> {
        match input {
            Input::SubscribeIntent(query) => Some(Self {
                query: query.clone(),
            }),
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

    fn into_query(self) -> Query {
        self.query
    }
}

impl MultiListenerRuntime for SpiritDaemonRuntime {
    type Listener = SpiritListener;
    type RequestError = DaemonError;
    type StartError = ActorStartFailure;
    type StopError = ActorStopFailure;

    fn start(&mut self) -> Result<(), Self::StartError> {
        self.engine.start()
    }

    fn stop(&mut self) -> Result<(), Self::StopError> {
        self.engine.stop()
    }

    fn handle_stream(
        &mut self,
        listener: SpiritListener,
        stream: UnixStream,
    ) -> Result<(), Self::RequestError> {
        match listener {
            SpiritListener::Working => self.handle_working_stream(stream),
            SpiritListener::Meta => self.handle_meta_stream(stream),
        }
    }
}

impl From<MultiListenerDaemonError<ActorStartFailure, ActorStopFailure>> for DaemonError {
    fn from(error: MultiListenerDaemonError<ActorStartFailure, ActorStopFailure>) -> Self {
        match error {
            MultiListenerDaemonError::Listener(error) => Self::Listener(error),
            MultiListenerDaemonError::Start(error) => Self::ActorStart(error),
            MultiListenerDaemonError::Stop(error) => Self::ActorStop(error),
        }
    }
}
