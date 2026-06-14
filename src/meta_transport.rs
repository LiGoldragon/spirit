//! Owner-only meta-signal wire transport.
//!
//! The meta-signal contract (`meta-signal-spirit/schema/meta-signal.schema`) is
//! emitted as a signal-frame wire contract: this `schema-rust-next` pin emits the
//! `encode_signal_frame` / `decode_signal_frame` short-header frame codec on
//! the meta `Input` / `Output` roots, identical to the working signal plane.
//! So `MetaSignalTransport` reuses the schema-emitted codec directly (the
//! previously hand-written `encode_meta_frame` / `decode_meta_frame` + route
//! enums are retired) over the same `triad_runtime::LengthPrefixedCodec`
//! length-prefix framing the working `SignalTransport` uses.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
};

use thiserror::Error;
use triad_runtime::{FrameBody as LengthPrefixedFrameBody, FrameError, LengthPrefixedCodec};

pub use crate::schema::meta_signal::{
    Configure, Input as MetaInput, InputRoute as MetaInputRoute, Output as MetaOutput,
    OutputRoute as MetaOutputRoute, SignalFrameError as MetaFrameError,
};

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
        self.exchange(&MetaInput::configure(request.into_payload()))
    }

    pub fn write_input(&mut self, request: &MetaInput) -> Result<(), MetaTransportError> {
        self.write_frame(request.encode_signal_frame()?)
    }

    pub fn read_input(&mut self) -> Result<(MetaInputRoute, MetaInput), MetaTransportError> {
        Ok(MetaInput::decode_signal_frame(&self.read_frame()?)?)
    }

    pub fn write_output(&mut self, reply: &MetaOutput) -> Result<(), MetaTransportError> {
        self.write_frame(reply.encode_signal_frame()?)
    }

    pub fn read_output(&mut self) -> Result<(MetaOutputRoute, MetaOutput), MetaTransportError> {
        Ok(MetaOutput::decode_signal_frame(&self.read_frame()?)?)
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
