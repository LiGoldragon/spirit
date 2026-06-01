use std::{
    fs,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::Arc,
};

use crate::{
    Configuration, Engine, StoreError,
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
