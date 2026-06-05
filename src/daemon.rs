use std::os::unix::net::UnixStream;

use thiserror::Error;
use triad_runtime::{
    ArgumentError, ComponentArgument, ComponentCommand, DaemonRuntime, ListenerError,
    RequestErrorLog, SingleListenerDaemon, SingleListenerDaemonError,
};

use crate::{
    ActorStartFailure, ActorStopFailure, Configuration, ConfigurationError, Engine, StoreError,
    store::Store,
    transport::{SignalTransport, TransportError},
};

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
        SingleListenerDaemon::new(
            self.configuration.socket_path(),
            runtime,
            RequestErrorLog::new("spirit-daemon"),
        )
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
}

impl SpiritDaemonRuntime {
    fn new(engine: Engine) -> Self {
        Self { engine }
    }

    fn handle_stream(&self, stream: UnixStream) -> Result<(), DaemonError> {
        let mut transport = SignalTransport::new(stream);
        let (_route, input) = transport.read_input()?;
        let output = self.engine.handle(input);
        transport.write_output(output.root())?;
        Ok(())
    }
}

impl DaemonRuntime for SpiritDaemonRuntime {
    type RequestError = DaemonError;
    type StartError = ActorStartFailure;
    type StopError = ActorStopFailure;

    fn start(&mut self) -> Result<(), Self::StartError> {
        self.engine.start()
    }

    fn stop(&mut self) -> Result<(), Self::StopError> {
        self.engine.stop()
    }

    fn handle_stream(&mut self, stream: UnixStream) -> Result<(), Self::RequestError> {
        SpiritDaemonRuntime::handle_stream(self, stream)
    }
}

impl From<SingleListenerDaemonError<ActorStartFailure, ActorStopFailure>> for DaemonError {
    fn from(error: SingleListenerDaemonError<ActorStartFailure, ActorStopFailure>) -> Self {
        match error {
            SingleListenerDaemonError::Listener(error) => Self::Listener(error),
            SingleListenerDaemonError::Start(error) => Self::ActorStart(error),
            SingleListenerDaemonError::Stop(error) => Self::ActorStop(error),
        }
    }
}
