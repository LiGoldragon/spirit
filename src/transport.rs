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

pub fn exchange(
    socket_path: impl AsRef<Path>,
    input: &Input,
) -> Result<(OutputRoute, Output), TransportError> {
    let mut stream = UnixStream::connect(socket_path)?;
    write_input(&mut stream, input)?;
    read_output(&mut stream)
}

pub fn write_input(writer: &mut impl Write, input: &Input) -> Result<(), TransportError> {
    write_frame(writer, input.encode_signal_frame()?)
}

pub fn read_input(reader: &mut impl Read) -> Result<(InputRoute, Input), TransportError> {
    Ok(Input::decode_signal_frame(&read_frame(reader)?)?)
}

pub fn write_output(writer: &mut impl Write, output: &Output) -> Result<(), TransportError> {
    write_frame(writer, output.encode_signal_frame()?)
}

pub fn read_output(reader: &mut impl Read) -> Result<(OutputRoute, Output), TransportError> {
    Ok(Output::decode_signal_frame(&read_frame(reader)?)?)
}

fn write_frame(writer: &mut impl Write, frame: Vec<u8>) -> Result<(), TransportError> {
    let length = u32::try_from(frame.len())
        .map_err(|_| TransportError::FrameTooLarge { found: frame.len() })?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

fn read_frame(reader: &mut impl Read) -> Result<Vec<u8>, TransportError> {
    let mut length_bytes = [0_u8; LENGTH_PREFIX_BYTE_COUNT];
    reader.read_exact(&mut length_bytes)?;
    let length = u32::from_be_bytes(length_bytes) as usize;
    let mut frame = vec![0_u8; length];
    reader.read_exact(&mut frame)?;
    Ok(frame)
}
