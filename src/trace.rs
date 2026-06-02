use std::{
    fmt,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::TraceEvent;

const LENGTH_PREFIX_BYTE_COUNT: usize = 4;

#[derive(Clone, Debug)]
pub struct TraceLog {
    destination: TraceDestination,
}

#[derive(Clone, Debug, Default)]
enum TraceDestination {
    #[default]
    Disabled,
    Recording(Arc<Mutex<Vec<TraceEvent>>>),
    Socket(TraceSocketPath),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceSocketPath {
    path: PathBuf,
}

pub struct TraceSocketListener {
    listener: UnixListener,
    path: PathBuf,
}

#[derive(Debug)]
pub enum TraceError {
    Io(std::io::Error),
    ArchiveEncode,
    ArchiveDecode,
    FrameTooLarge { found: usize },
}

impl Default for TraceLog {
    fn default() -> Self {
        Self::recording()
    }
}

impl TraceLog {
    pub fn disabled() -> Self {
        Self {
            destination: TraceDestination::Disabled,
        }
    }

    pub fn recording() -> Self {
        Self {
            destination: TraceDestination::Recording(Arc::new(Mutex::new(Vec::new()))),
        }
    }

    pub fn socket(path: impl Into<PathBuf>) -> Self {
        Self {
            destination: TraceDestination::Socket(TraceSocketPath::new(path)),
        }
    }

    pub fn events(&self) -> Vec<TraceEvent> {
        match &self.destination {
            TraceDestination::Recording(events) => events.lock().expect("trace event lock").clone(),
            TraceDestination::Disabled | TraceDestination::Socket(_) => Vec::new(),
        }
    }

    pub fn record(&self, event: TraceEvent) {
        match &self.destination {
            TraceDestination::Disabled => {}
            TraceDestination::Recording(events) => {
                events.lock().expect("trace event lock").push(event);
            }
            TraceDestination::Socket(path) => {
                if let Err(error) = path.write_event(&event) {
                    eprintln!("spirit-next trace: {error}");
                }
            }
        }
    }
}

impl TraceEvent {
    pub fn to_frame(&self) -> Result<Vec<u8>, TraceError> {
        let archive =
            rkyv::to_bytes::<rkyv::rancor::Error>(self).map_err(|_| TraceError::ArchiveEncode)?;
        let length = u32::try_from(archive.len()).map_err(|_| TraceError::FrameTooLarge {
            found: archive.len(),
        })?;
        let mut frame = Vec::with_capacity(LENGTH_PREFIX_BYTE_COUNT + archive.len());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(&archive);
        Ok(frame)
    }

    pub fn from_frame(frame: &[u8]) -> Result<Self, TraceError> {
        rkyv::from_bytes::<Self, rkyv::rancor::Error>(frame).map_err(|_| TraceError::ArchiveDecode)
    }

    pub fn write_to(&self, stream: &mut UnixStream) -> Result<(), TraceError> {
        stream.write_all(&self.to_frame()?)?;
        Ok(())
    }

    pub fn read_from(stream: &mut UnixStream) -> Result<Self, TraceError> {
        let mut length_bytes = [0_u8; LENGTH_PREFIX_BYTE_COUNT];
        stream.read_exact(&mut length_bytes)?;
        let length = u32::from_be_bytes(length_bytes) as usize;
        let mut frame = vec![0_u8; length];
        stream.read_exact(&mut frame)?;
        Self::from_frame(&frame)
    }
}

impl TraceSocketPath {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }

    pub fn write_event(&self, event: &TraceEvent) -> Result<(), TraceError> {
        let mut stream = UnixStream::connect(&self.path)?;
        event.write_to(&mut stream)
    }
}

impl TraceSocketListener {
    pub fn bind(path: impl Into<PathBuf>) -> Result<Self, TraceError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(TraceError::Io(error)),
        }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, path })
    }

    pub fn collect_for(&self, duration: Duration) -> Result<Vec<TraceEvent>, TraceError> {
        let deadline = Instant::now() + duration;
        let mut events = Vec::new();
        while Instant::now() < deadline {
            match self.listener.accept() {
                Ok((mut stream, _address)) => events.push(TraceEvent::read_from(&mut stream)?),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(TraceError::Io(error)),
            }
        }
        Ok(events)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TraceSocketListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl fmt::Display for TraceEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "trace IO error: {error}"),
            Self::ArchiveEncode => formatter.write_str("failed to encode trace event"),
            Self::ArchiveDecode => formatter.write_str("failed to decode trace event"),
            Self::FrameTooLarge { found } => {
                write!(formatter, "trace frame is too large: {found} bytes")
            }
        }
    }
}

impl std::error::Error for TraceError {}

impl From<std::io::Error> for TraceError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
