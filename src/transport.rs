use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
};

use thiserror::Error;
use triad_runtime::{FrameBody as LengthPrefixedFrameBody, FrameError, LengthPrefixedCodec};

use crate::schema::signal::{
    Frame as StreamingFrame, FrameBody as StreamingFrameBody, Input, InputRoute, IntentEvent,
    Output, OutputRoute, SignalFrameError,
};

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("transport IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("signal frame error: {0}")]
    SignalFrame(#[from] SignalFrameError),

    #[error("streaming signal frame error: {0}")]
    StreamingFrame(#[from] signal_frame::FrameError),

    #[error("transport frame error: {0}")]
    Frame(#[from] FrameError),

    #[error("expected subscription event frame")]
    ExpectedSubscriptionEvent,
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

    pub fn read_streaming_frame(&mut self) -> Result<StreamingFrame, TransportError> {
        Ok(StreamingFrame::decode(&self.read_frame()?)?)
    }

    pub fn read_subscription_event(&mut self) -> Result<IntentEvent, TransportError> {
        match self.read_streaming_frame()?.into_body() {
            StreamingFrameBody::SubscriptionEvent { event, .. } => Ok(event),
            StreamingFrameBody::HandshakeRequest(_)
            | StreamingFrameBody::HandshakeReply(_)
            | StreamingFrameBody::Request { .. }
            | StreamingFrameBody::Reply { .. } => Err(TransportError::ExpectedSubscriptionEvent),
        }
    }

    fn write_frame(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        LengthPrefixedCodec::default()
            .write_body(&mut self.stream, &LengthPrefixedFrameBody::new(frame))?;
        self.stream.flush()?;
        Ok(())
    }

    fn read_frame(&mut self) -> Result<Vec<u8>, TransportError> {
        Ok(LengthPrefixedCodec::default()
            .read_body(&mut self.stream)?
            .into_bytes())
    }
}
