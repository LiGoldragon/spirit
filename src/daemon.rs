use std::{
    fs,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::Arc,
};

use crate::{
    Configuration, Engine,
    transport::{self, TransportError},
};

#[derive(Debug)]
pub enum DaemonError {
    Io(std::io::Error),
    Transport(TransportError),
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "daemon IO error: {error}"),
            Self::Transport(error) => write!(formatter, "daemon transport error: {error}"),
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

pub fn run_daemon(configuration: Configuration) -> Result<(), DaemonError> {
    if let Some(parent) = configuration.socket_path.parent() {
        fs::create_dir_all(parent)?;
    }
    remove_stale_socket(&configuration.socket_path)?;
    let listener = UnixListener::bind(&configuration.socket_path)?;
    let engine = Arc::new(Engine::default());
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let engine = Arc::clone(&engine);
                if let Err(error) = handle_stream(stream, &engine) {
                    eprintln!("spirit-next-daemon: {error}");
                }
            }
            Err(error) => return Err(DaemonError::Io(error)),
        }
    }
    Ok(())
}

fn handle_stream(mut stream: UnixStream, engine: &Engine) -> Result<(), DaemonError> {
    let (_route, input) = transport::read_input(&mut stream)?;
    let output = engine.handle(input);
    transport::write_output(&mut stream, &output)?;
    Ok(())
}

fn remove_stale_socket(path: &Path) -> Result<(), DaemonError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DaemonError::Io(error)),
    }
}
