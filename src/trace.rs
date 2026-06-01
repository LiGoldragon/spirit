use std::{
    fmt,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::{
    Input, NexusInput, NexusOutput, OriginRoute, Output, SemaReadInput, SemaReadOutput,
    SemaWriteInput, SemaWriteOutput, ValidationError,
};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceActor {
    Signal,
    Nexus,
    Sema,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceInterface {
    SignalAdmission,
    SignalEngine,
    NexusEngine,
    SemaEngine,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    SignalAdmitted {
        origin_route: OriginRoute,
        input: Input,
    },
    SignalTriaged {
        origin_route: OriginRoute,
        input: Input,
        output: NexusInput,
    },
    SignalRejected {
        origin_route: OriginRoute,
        validation_error: ValidationError,
    },
    SignalReplied {
        origin_route: OriginRoute,
        output: Output,
    },
    NexusEntered {
        origin_route: OriginRoute,
        input: NexusInput,
    },
    NexusDecided {
        origin_route: OriginRoute,
        output: NexusOutput,
    },
    SemaWriteApplied {
        origin_route: OriginRoute,
        input: SemaWriteInput,
        output: SemaWriteOutput,
    },
    SemaReadObserved {
        origin_route: OriginRoute,
        input: SemaReadInput,
        output: SemaReadOutput,
    },
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

    pub fn name(&self) -> &'static str {
        match self {
            Self::SignalAdmitted { .. } => "SignalAdmitted",
            Self::SignalTriaged { .. } => "SignalTriaged",
            Self::SignalRejected { .. } => "SignalRejected",
            Self::SignalReplied { .. } => "SignalReplied",
            Self::NexusEntered { .. } => "NexusEntered",
            Self::NexusDecided { .. } => "NexusDecided",
            Self::SemaWriteApplied { .. } => "SemaWriteApplied",
            Self::SemaReadObserved { .. } => "SemaReadObserved",
        }
    }

    pub fn actor(&self) -> TraceActor {
        match self {
            Self::SignalAdmitted { .. }
            | Self::SignalTriaged { .. }
            | Self::SignalRejected { .. }
            | Self::SignalReplied { .. } => TraceActor::Signal,
            Self::NexusEntered { .. } | Self::NexusDecided { .. } => TraceActor::Nexus,
            Self::SemaWriteApplied { .. } | Self::SemaReadObserved { .. } => TraceActor::Sema,
        }
    }

    pub fn interface(&self) -> TraceInterface {
        match self {
            Self::SignalAdmitted { .. } | Self::SignalRejected { .. } => {
                TraceInterface::SignalAdmission
            }
            Self::SignalTriaged { .. } | Self::SignalReplied { .. } => TraceInterface::SignalEngine,
            Self::NexusEntered { .. } | Self::NexusDecided { .. } => TraceInterface::NexusEngine,
            Self::SemaWriteApplied { .. } | Self::SemaReadObserved { .. } => {
                TraceInterface::SemaEngine
            }
        }
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
        match self {
            Self::SignalAdmitted {
                origin_route,
                input,
            } => write!(
                formatter,
                "{} actor={} interface={} origin_route={origin_route:?} input={input:?}",
                self.name(),
                self.actor(),
                self.interface()
            ),
            Self::SignalTriaged {
                origin_route,
                input,
                output,
            } => write!(
                formatter,
                "{} actor={} interface={} origin_route={origin_route:?} input={input:?} output={output:?}",
                self.name(),
                self.actor(),
                self.interface()
            ),
            Self::SignalRejected {
                origin_route,
                validation_error,
            } => write!(
                formatter,
                "{} actor={} interface={} origin_route={origin_route:?} validation_error={validation_error:?}",
                self.name(),
                self.actor(),
                self.interface()
            ),
            Self::SignalReplied {
                origin_route,
                output,
            } => write!(
                formatter,
                "{} actor={} interface={} origin_route={origin_route:?} output={output:?}",
                self.name(),
                self.actor(),
                self.interface()
            ),
            Self::NexusEntered {
                origin_route,
                input,
            } => write!(
                formatter,
                "{} actor={} interface={} origin_route={origin_route:?} input={input:?}",
                self.name(),
                self.actor(),
                self.interface()
            ),
            Self::NexusDecided {
                origin_route,
                output,
            } => write!(
                formatter,
                "{} actor={} interface={} origin_route={origin_route:?} output={output:?}",
                self.name(),
                self.actor(),
                self.interface()
            ),
            Self::SemaWriteApplied {
                origin_route,
                input,
                output,
            } => write!(
                formatter,
                "{} actor={} interface={} origin_route={origin_route:?} input={input:?} output={output:?}",
                self.name(),
                self.actor(),
                self.interface()
            ),
            Self::SemaReadObserved {
                origin_route,
                input,
                output,
            } => write!(
                formatter,
                "{} actor={} interface={} origin_route={origin_route:?} input={input:?} output={output:?}",
                self.name(),
                self.actor(),
                self.interface()
            ),
        }
    }
}

impl fmt::Display for TraceActor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signal => formatter.write_str("Signal"),
            Self::Nexus => formatter.write_str("Nexus"),
            Self::Sema => formatter.write_str("Sema"),
        }
    }
}

impl fmt::Display for TraceInterface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignalAdmission => formatter.write_str("SignalAdmission"),
            Self::SignalEngine => formatter.write_str("SignalEngine"),
            Self::NexusEngine => formatter.write_str("NexusEngine"),
            Self::SemaEngine => formatter.write_str("SemaEngine"),
        }
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
