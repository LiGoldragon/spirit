use std::{
    env, fs,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::Arc,
};

use crate::{
    Configuration, ConfigurationError, Engine, StoreError,
    store::Store,
    transport::{SignalTransport, TransportError},
};

#[cfg(feature = "testing-trace")]
use crate::TraceLog;

#[derive(Debug)]
pub enum DaemonError {
    Io(std::io::Error),
    Transport(TransportError),
    Store(StoreError),
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "daemon IO error: {error}"),
            Self::Transport(error) => write!(formatter, "daemon transport error: {error}"),
            Self::Store(error) => write!(formatter, "daemon sema store error: {error}"),
        }
    }
}

impl std::error::Error for DaemonError {}

impl From<std::io::Error> for DaemonError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<TransportError> for DaemonError {
    fn from(value: TransportError) -> Self {
        Self::Transport(value)
    }
}

impl From<StoreError> for DaemonError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

#[derive(Debug)]
pub enum DaemonCommandError {
    ArgumentCount { count: usize },
    Configuration(ConfigurationError),
    Daemon(DaemonError),
}

impl std::fmt::Display for DaemonCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArgumentCount { count } => write!(
                formatter,
                "expected exactly one binary configuration path, received {count}"
            ),
            Self::Configuration(error) => write!(formatter, "{error}"),
            Self::Daemon(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for DaemonCommandError {}

impl From<ConfigurationError> for DaemonCommandError {
    fn from(value: ConfigurationError) -> Self {
        Self::Configuration(value)
    }
}

impl From<DaemonError> for DaemonCommandError {
    fn from(value: DaemonError) -> Self {
        Self::Daemon(value)
    }
}

pub struct DaemonCommand {
    arguments: Vec<String>,
}

impl DaemonCommand {
    pub fn from_environment() -> Self {
        Self {
            arguments: env::args().skip(1).collect(),
        }
    }

    pub fn from_arguments<Arguments, Argument>(arguments: Arguments) -> Self
    where
        Arguments: IntoIterator<Item = Argument>,
        Argument: Into<String>,
    {
        Self {
            arguments: arguments.into_iter().map(Into::into).collect(),
        }
    }

    pub fn configuration(&self) -> Result<Configuration, DaemonCommandError> {
        Configuration::from_binary_path(self.single_argument()?).map_err(Into::into)
    }

    pub fn run(&self) -> Result<(), DaemonCommandError> {
        Daemon::new(self.configuration()?).run().map_err(Into::into)
    }

    fn single_argument(&self) -> Result<&str, DaemonCommandError> {
        match self.arguments.as_slice() {
            [argument] => Ok(argument),
            _ => Err(DaemonCommandError::ArgumentCount {
                count: self.arguments.len(),
            }),
        }
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
        let engine = Arc::new(self.engine()?);
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
