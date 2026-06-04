use std::{
    fs,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::Arc,
};

use thiserror::Error;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand};

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
        if let Some(parent) = self.configuration.socket_path().parent() {
            fs::create_dir_all(parent)?;
        }
        self.remove_stale_socket()?;
        let listener = UnixListener::bind(self.configuration.socket_path())?;
        let mut engine = self.engine()?;
        engine.start()?;
        let engine = Arc::new(engine);
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let engine = Arc::clone(&engine);
                    if let Err(error) = self.handle_stream(stream, &engine) {
                        eprintln!("spirit-next-daemon: {error}");
                    }
                }
                Err(error) => return Err(DaemonError::Io(error)),
            }
        }
        Ok(())
    }

    fn engine(&self) -> Result<Engine, DaemonError> {
        #[cfg(feature = "testing-trace")]
        {
            let trace_log = self
                .configuration
                .trace_socket_path()
                .map(TraceLog::socket)
                .unwrap_or_default();
            let store =
                Store::open_with_trace(self.configuration.database_path(), trace_log.clone())?;
            Ok(Engine::new_with_trace(store, trace_log))
        }
        #[cfg(not(feature = "testing-trace"))]
        {
            let store = Store::open(self.configuration.database_path())?;
            Ok(Engine::new(store))
        }
    }

    fn handle_stream(&self, stream: UnixStream, engine: &Engine) -> Result<(), DaemonError> {
        let mut transport = SignalTransport::new(stream);
        let (_route, input) = transport.read_input()?;
        let output = engine.handle(input);
        transport.write_output(output.root())?;
        Ok(())
    }

    fn remove_stale_socket(&self) -> Result<(), DaemonError> {
        let path = SocketPath::new(self.configuration.socket_path());
        path.remove_stale()
    }
}

struct SocketPath<'path> {
    path: &'path Path,
}

impl<'path> SocketPath<'path> {
    fn new(path: &'path Path) -> Self {
        Self { path }
    }

    fn remove_stale(&self) -> Result<(), DaemonError> {
        match fs::remove_file(self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DaemonError::Io(error)),
        }
    }
}
