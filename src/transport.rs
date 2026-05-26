use std::{
    fmt,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
};

use crate::{Input, Output, generated::short_header};

const SHORT_HEADER_BYTE_COUNT: usize = 8;
const LENGTH_PREFIX_BYTE_COUNT: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputRoute {
    Record,
    Observe,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputRoute {
    RecordAccepted,
    RecordsObserved,
    Error,
}

#[derive(Debug)]
pub enum TransportError {
    Io(std::io::Error),
    ArchiveEncode,
    ArchiveDecode,
    FrameTooShort { found: usize },
    FrameTooLarge { found: usize },
    UnknownInputHeader { header: u64 },
    UnknownOutputHeader { header: u64 },
    HeaderMismatch { expected: u64, found: u64 },
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "transport IO error: {error}"),
            Self::ArchiveEncode => formatter.write_str("failed to encode rkyv archive"),
            Self::ArchiveDecode => formatter.write_str("failed to decode rkyv archive"),
            Self::FrameTooShort { found } => {
                write!(formatter, "frame too short: {found} bytes")
            }
            Self::FrameTooLarge { found } => {
                write!(formatter, "frame too large for u32 prefix: {found} bytes")
            }
            Self::UnknownInputHeader { header } => {
                write!(formatter, "unknown input short header 0x{header:016X}")
            }
            Self::UnknownOutputHeader { header } => {
                write!(formatter, "unknown output short header 0x{header:016X}")
            }
            Self::HeaderMismatch { expected, found } => write!(
                formatter,
                "decoded payload header mismatch: expected 0x{expected:016X}, found 0x{found:016X}"
            ),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<std::io::Error> for TransportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
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
    write_frame(writer, encode_input_frame(input)?)
}

pub fn read_input(reader: &mut impl Read) -> Result<(InputRoute, Input), TransportError> {
    decode_input_frame(&read_frame(reader)?)
}

pub fn write_output(writer: &mut impl Write, output: &Output) -> Result<(), TransportError> {
    write_frame(writer, encode_output_frame(output)?)
}

pub fn read_output(reader: &mut impl Read) -> Result<(OutputRoute, Output), TransportError> {
    decode_output_frame(&read_frame(reader)?)
}

pub fn input_short_header(input: &Input) -> u64 {
    match input {
        Input::Record(_) => short_header::INPUT_RECORD,
        Input::Observe(_) => short_header::INPUT_OBSERVE,
    }
}

pub fn output_short_header(output: &Output) -> u64 {
    match output {
        Output::RecordAccepted(_) => short_header::OUTPUT_RECORD_ACCEPTED,
        Output::RecordsObserved(_) => short_header::OUTPUT_RECORDS_OBSERVED,
        Output::Error(_) => short_header::OUTPUT_ERROR,
    }
}

pub fn input_route(header: u64) -> Result<InputRoute, TransportError> {
    match header {
        short_header::INPUT_RECORD => Ok(InputRoute::Record),
        short_header::INPUT_OBSERVE => Ok(InputRoute::Observe),
        _ => Err(TransportError::UnknownInputHeader { header }),
    }
}

pub fn output_route(header: u64) -> Result<OutputRoute, TransportError> {
    match header {
        short_header::OUTPUT_RECORD_ACCEPTED => Ok(OutputRoute::RecordAccepted),
        short_header::OUTPUT_RECORDS_OBSERVED => Ok(OutputRoute::RecordsObserved),
        short_header::OUTPUT_ERROR => Ok(OutputRoute::Error),
        _ => Err(TransportError::UnknownOutputHeader { header }),
    }
}

fn encode_input_frame(input: &Input) -> Result<Vec<u8>, TransportError> {
    encode_frame(input_short_header(input), input)
}

fn encode_output_frame(output: &Output) -> Result<Vec<u8>, TransportError> {
    encode_frame(output_short_header(output), output)
}

fn encode_frame<Value>(header: u64, value: &Value) -> Result<Vec<u8>, TransportError>
where
    Value: rkyv::Archive
        + for<'archive> rkyv::Serialize<
            rkyv::api::high::HighSerializer<
                rkyv::util::AlignedVec,
                rkyv::ser::allocator::ArenaHandle<'archive>,
                rkyv::rancor::Error,
            >,
        >,
{
    let archive =
        rkyv::to_bytes::<rkyv::rancor::Error>(value).map_err(|_| TransportError::ArchiveEncode)?;
    let mut frame = Vec::with_capacity(SHORT_HEADER_BYTE_COUNT + archive.len());
    frame.extend_from_slice(&header.to_le_bytes());
    frame.extend_from_slice(&archive);
    Ok(frame)
}

fn decode_input_frame(frame: &[u8]) -> Result<(InputRoute, Input), TransportError> {
    let (header, body) = split_header(frame)?;
    let route = input_route(header)?;
    let input = rkyv::from_bytes::<Input, rkyv::rancor::Error>(body)
        .map_err(|_| TransportError::ArchiveDecode)?;
    let expected = input_short_header(&input);
    if expected != header {
        return Err(TransportError::HeaderMismatch {
            expected,
            found: header,
        });
    }
    Ok((route, input))
}

fn decode_output_frame(frame: &[u8]) -> Result<(OutputRoute, Output), TransportError> {
    let (header, body) = split_header(frame)?;
    let route = output_route(header)?;
    let output = rkyv::from_bytes::<Output, rkyv::rancor::Error>(body)
        .map_err(|_| TransportError::ArchiveDecode)?;
    let expected = output_short_header(&output);
    if expected != header {
        return Err(TransportError::HeaderMismatch {
            expected,
            found: header,
        });
    }
    Ok((route, output))
}

fn split_header(frame: &[u8]) -> Result<(u64, &[u8]), TransportError> {
    if frame.len() < SHORT_HEADER_BYTE_COUNT {
        return Err(TransportError::FrameTooShort { found: frame.len() });
    }
    let mut bytes = [0_u8; SHORT_HEADER_BYTE_COUNT];
    bytes.copy_from_slice(&frame[..SHORT_HEADER_BYTE_COUNT]);
    Ok((u64::from_le_bytes(bytes), &frame[SHORT_HEADER_BYTE_COUNT..]))
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
