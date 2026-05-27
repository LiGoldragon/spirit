use std::{
    fmt,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
};

use crate::{Input, InputRoute, Output, OutputRoute, SignalFrameError};

const LENGTH_PREFIX_BYTE_COUNT: usize = 4;

#[derive(Debug)]
pub enum TransportError {
    Io(std::io::Error),
    SignalFrame(SignalFrameError),
    FrameTooLarge { found: usize },
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "transport IO error: {error}"),
            Self::SignalFrame(error) => write!(formatter, "signal frame error: {error}"),
            Self::FrameTooLarge { found } => {
                write!(formatter, "frame too large for u32 prefix: {found} bytes")
            }
        }
    }
}

impl std::error::Error for TransportError {}

impl From<std::io::Error> for TransportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<SignalFrameError> for TransportError {
    fn from(value: SignalFrameError) -> Self {
        Self::SignalFrame(value)
    }
}

pub struct SignalTransport<Stream> {
    stream: Stream,
}

impl SignalTransport<UnixStream> {
    pub fn connect(socket_path: impl AsRef<Path>) -> Result<Self, TransportError> {
        Ok(Self::new(UnixStream::connect(socket_path)?))
    }
}

impl<Stream> SignalTransport<Stream>
where
    Stream: Read + Write,
{
    pub fn new(stream: Stream) -> Self {
        Self { stream }
    }

    pub fn exchange(&mut self, input: &Input) -> Result<(OutputRoute, Output), TransportError> {
        self.write_input(input)?;
        self.read_output()
    }

    pub fn write_input(&mut self, input: &Input) -> Result<(), TransportError> {
        self.write_frame(input.encode_signal_frame()?)
    }

    pub fn read_input(&mut self) -> Result<(InputRoute, Input), TransportError> {
        Ok(Input::decode_signal_frame(&self.read_frame()?)?)
    }

    pub fn write_output(&mut self, output: &Output) -> Result<(), TransportError> {
        self.write_frame(output.encode_signal_frame()?)
    }

    pub fn read_output(&mut self) -> Result<(OutputRoute, Output), TransportError> {
        Ok(Output::decode_signal_frame(&self.read_frame()?)?)
    }

    fn write_frame(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        let length = u32::try_from(frame.len())
            .map_err(|_| TransportError::FrameTooLarge { found: frame.len() })?;
        self.stream.write_all(&length.to_be_bytes())?;
        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        Ok(())
    }

    fn read_frame(&mut self) -> Result<Vec<u8>, TransportError> {
        let mut length_bytes = [0_u8; LENGTH_PREFIX_BYTE_COUNT];
        self.stream.read_exact(&mut length_bytes)?;
        let length = u32::from_be_bytes(length_bytes) as usize;
        let mut frame = vec![0_u8; length];
        self.stream.read_exact(&mut frame)?;
        Ok(frame)
    }
}
