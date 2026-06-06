//! Owner-only meta-signal wire transport.
//!
//! The meta-signal contract (`schema/meta-signal.schema`) is emitted as a
//! `RustEmissionTarget::WireContract` module: it carries the `Configure`
//! `Input` / `Output` roots, their records, the rkyv derives, and the
//! per-root `short_header` constants — but the wire-contract target in this
//! `schema-rust-next` pin does NOT emit the `encode_signal_frame` /
//! `decode_signal_frame` frame codec that the signal runtime target emits.
//!
//! So the frame codec lives here as methods on the schema-emitted `Input` /
//! `Output` nouns, byte-identical to the signal plane's short-header frame
//! (8-byte little-endian short header followed by the rkyv archive, with the
//! header re-verified against the decoded payload). `MetaSignalTransport`
//! reuses the exact same `triad_runtime::LengthPrefixedCodec` length-prefix
//! framing the working `SignalTransport` uses; only the per-root codec differs.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
};

use thiserror::Error;
use triad_runtime::{FrameBody as LengthPrefixedFrameBody, FrameError, LengthPrefixedCodec};

use crate::schema::meta_signal::short_header;
pub use crate::schema::meta_signal::{
    Configure, Input as MetaInput, Output as MetaOutput,
};

/// The 8-byte little-endian short header that prefixes every meta frame,
/// matching the signal plane's frame layout.
const META_SHORT_HEADER_BYTE_COUNT: usize = 8;

/// Which meta `Input` root a decoded frame carries. The meta contract has a
/// single owner-only operation today, so this enum has one variant; it exists
/// so the decode path returns a typed route alongside the value exactly like
/// the signal plane's `InputRoute`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetaInputRoute {
    Configure,
}

/// Which meta `Output` root a decoded frame carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetaOutputRoute {
    Configured,
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum MetaFrameError {
    #[error("failed to encode rkyv archive")]
    ArchiveEncode,

    #[error("failed to decode rkyv archive")]
    ArchiveDecode,

    #[error("meta frame too short: {found} bytes")]
    FrameTooShort { found: usize },

    #[error("unknown {root_enum} short header 0x{header:016X}")]
    UnknownHeader {
        root_enum: &'static str,
        header: u64,
    },

    #[error("decoded payload header mismatch: expected 0x{expected:016X}, found 0x{found:016X}")]
    HeaderMismatch { expected: u64, found: u64 },
}

impl MetaInput {
    pub fn route(&self) -> MetaInputRoute {
        match self {
            Self::Configure(_) => MetaInputRoute::Configure,
        }
    }

    pub fn short_header(&self) -> u64 {
        match self {
            Self::Configure(_) => short_header::INPUT_CONFIGURE,
        }
    }

    pub fn route_from_short_header(header: u64) -> Result<MetaInputRoute, MetaFrameError> {
        match header {
            short_header::INPUT_CONFIGURE => Ok(MetaInputRoute::Configure),
            _ => Err(MetaFrameError::UnknownHeader {
                root_enum: "Input",
                header,
            }),
        }
    }

    pub fn encode_meta_frame(&self) -> Result<Vec<u8>, MetaFrameError> {
        let archive =
            rkyv::to_bytes::<rkyv::rancor::Error>(self).map_err(|_| MetaFrameError::ArchiveEncode)?;
        let mut frame = Vec::with_capacity(META_SHORT_HEADER_BYTE_COUNT + archive.len());
        frame.extend_from_slice(&self.short_header().to_le_bytes());
        frame.extend_from_slice(&archive);
        Ok(frame)
    }

    pub fn decode_meta_frame(frame: &[u8]) -> Result<(MetaInputRoute, Self), MetaFrameError> {
        if frame.len() < META_SHORT_HEADER_BYTE_COUNT {
            return Err(MetaFrameError::FrameTooShort { found: frame.len() });
        }
        let mut header_bytes = [0_u8; META_SHORT_HEADER_BYTE_COUNT];
        header_bytes.copy_from_slice(&frame[..META_SHORT_HEADER_BYTE_COUNT]);
        let header = u64::from_le_bytes(header_bytes);
        let route = Self::route_from_short_header(header)?;
        let value =
            rkyv::from_bytes::<Self, rkyv::rancor::Error>(&frame[META_SHORT_HEADER_BYTE_COUNT..])
                .map_err(|_| MetaFrameError::ArchiveDecode)?;
        let expected = value.short_header();
        if expected != header {
            return Err(MetaFrameError::HeaderMismatch {
                expected,
                found: header,
            });
        }
        Ok((route, value))
    }
}

impl MetaOutput {
    pub fn route(&self) -> MetaOutputRoute {
        match self {
            Self::Configured(_) => MetaOutputRoute::Configured,
            Self::Rejected(_) => MetaOutputRoute::Rejected,
        }
    }

    pub fn short_header(&self) -> u64 {
        match self {
            Self::Configured(_) => short_header::OUTPUT_CONFIGURED,
            Self::Rejected(_) => short_header::OUTPUT_REJECTED,
        }
    }

    pub fn route_from_short_header(header: u64) -> Result<MetaOutputRoute, MetaFrameError> {
        match header {
            short_header::OUTPUT_CONFIGURED => Ok(MetaOutputRoute::Configured),
            short_header::OUTPUT_REJECTED => Ok(MetaOutputRoute::Rejected),
            _ => Err(MetaFrameError::UnknownHeader {
                root_enum: "Output",
                header,
            }),
        }
    }

    pub fn encode_meta_frame(&self) -> Result<Vec<u8>, MetaFrameError> {
        let archive =
            rkyv::to_bytes::<rkyv::rancor::Error>(self).map_err(|_| MetaFrameError::ArchiveEncode)?;
        let mut frame = Vec::with_capacity(META_SHORT_HEADER_BYTE_COUNT + archive.len());
        frame.extend_from_slice(&self.short_header().to_le_bytes());
        frame.extend_from_slice(&archive);
        Ok(frame)
    }

    pub fn decode_meta_frame(frame: &[u8]) -> Result<(MetaOutputRoute, Self), MetaFrameError> {
        if frame.len() < META_SHORT_HEADER_BYTE_COUNT {
            return Err(MetaFrameError::FrameTooShort { found: frame.len() });
        }
        let mut header_bytes = [0_u8; META_SHORT_HEADER_BYTE_COUNT];
        header_bytes.copy_from_slice(&frame[..META_SHORT_HEADER_BYTE_COUNT]);
        let header = u64::from_le_bytes(header_bytes);
        let route = Self::route_from_short_header(header)?;
        let value =
            rkyv::from_bytes::<Self, rkyv::rancor::Error>(&frame[META_SHORT_HEADER_BYTE_COUNT..])
                .map_err(|_| MetaFrameError::ArchiveDecode)?;
        let expected = value.short_header();
        if expected != header {
            return Err(MetaFrameError::HeaderMismatch {
                expected,
                found: header,
            });
        }
        Ok((route, value))
    }
}

#[derive(Debug, Error)]
pub enum MetaTransportError {
    #[error("meta transport IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("meta frame error: {0}")]
    MetaFrame(#[from] MetaFrameError),

    #[error("meta transport frame error: {0}")]
    Frame(#[from] FrameError),
}

/// The owner-only meta transport over a connected stream: length-prefixed
/// frames carrying meta `Input` requests and meta `Output` replies. It mirrors
/// `SignalTransport` but is typed over the meta wire vocabulary so the working
/// and owner sockets stay distinct wire languages.
pub struct MetaSignalTransport<Stream> {
    stream: Stream,
}

impl MetaSignalTransport<UnixStream> {
    pub fn connect(socket_path: impl AsRef<Path>) -> Result<Self, MetaTransportError> {
        Ok(Self::new(UnixStream::connect(socket_path)?))
    }
}

impl<Stream> MetaSignalTransport<Stream>
where
    Stream: Read + Write,
{
    pub fn new(stream: Stream) -> Self {
        Self { stream }
    }

    pub fn exchange(
        &mut self,
        request: &MetaInput,
    ) -> Result<(MetaOutputRoute, MetaOutput), MetaTransportError> {
        self.write_input(request)?;
        self.read_output()
    }

    pub fn configure(
        &mut self,
        request: Configure,
    ) -> Result<(MetaOutputRoute, MetaOutput), MetaTransportError> {
        self.exchange(&MetaInput::configure(request))
    }

    pub fn write_input(&mut self, request: &MetaInput) -> Result<(), MetaTransportError> {
        self.write_frame(request.encode_meta_frame()?)
    }

    pub fn read_input(&mut self) -> Result<(MetaInputRoute, MetaInput), MetaTransportError> {
        Ok(MetaInput::decode_meta_frame(&self.read_frame()?)?)
    }

    pub fn write_output(&mut self, reply: &MetaOutput) -> Result<(), MetaTransportError> {
        self.write_frame(reply.encode_meta_frame()?)
    }

    pub fn read_output(&mut self) -> Result<(MetaOutputRoute, MetaOutput), MetaTransportError> {
        Ok(MetaOutput::decode_meta_frame(&self.read_frame()?)?)
    }

    fn write_frame(&mut self, frame: Vec<u8>) -> Result<(), MetaTransportError> {
        LengthPrefixedCodec::default()
            .write_body(&mut self.stream, &LengthPrefixedFrameBody::new(frame))?;
        self.stream.flush()?;
        Ok(())
    }

    fn read_frame(&mut self) -> Result<Vec<u8>, MetaTransportError> {
        Ok(LengthPrefixedCodec::default()
            .read_body(&mut self.stream)?
            .into_bytes())
    }
}
