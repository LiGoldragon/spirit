use std::{env, io::ErrorKind, os::unix::net::UnixStream};

use nota::NotaDecodeError;
use spirit::{
    SignalTransport, TransportError,
    schema::signal::{Input, Output},
};
use thiserror::Error;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand, FrameError};

#[cfg(feature = "testing-trace")]
use spirit::{TraceClient, TraceError};
#[cfg(feature = "testing-trace")]
use std::time::Duration;

fn main() {
    if let Err(error) = SpiritCli::from_environment().run() {
        eprintln!("spirit: {error}");
        std::process::exit(1);
    }
}

struct SpiritCli {
    command: ComponentCommand,
}

impl SpiritCli {
    fn from_environment() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
        }
    }

    fn run(&self) -> Result<(), SpiritCliError> {
        let source = self.source()?;
        let input = source.parse_input()?;
        let socket_path =
            env::var("SPIRIT_SOCKET").unwrap_or_else(|_| String::from("/tmp/spirit.sock"));
        #[cfg(feature = "testing-trace")]
        let trace_client =
            TraceClient::from_environment("SPIRIT_TRACE_SOCKET", Duration::from_millis(200))?;
        let opens_subscription = matches!(&input, Input::SubscribeIntent(_));
        let mut transport = SignalTransport::connect(socket_path)?;
        let (_route, output) = transport.exchange(&input)?;
        println!("{output}");
        if opens_subscription {
            self.print_subscription_events(&mut transport)?;
        }
        #[cfg(feature = "testing-trace")]
        trace_client.print_events(&mut std::io::stdout())?;
        Ok(())
    }

    fn print_subscription_events(
        &self,
        transport: &mut SignalTransport<UnixStream>,
    ) -> Result<(), SpiritCliError> {
        loop {
            match transport.read_subscription_event() {
                Ok(event) => println!("{}", Output::event(event)),
                Err(TransportError::Frame(FrameError::Io(error)))
                    if error.kind() == ErrorKind::UnexpectedEof =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn source(&self) -> Result<SpiritInputSource, SpiritCliError> {
        match self.command.nota_argument()? {
            ComponentArgument::InlineNota(argument) => {
                Ok(SpiritInputSource::new(argument.into_string()))
            }
            ComponentArgument::NotaFile(_) | ComponentArgument::SignalFile(_) => {
                Err(SpiritCliError::InlineNotaRequired)
            }
        }
    }
}

struct SpiritInputSource {
    text: String,
}

impl SpiritInputSource {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn parse_input(&self) -> Result<Input, NotaDecodeError> {
        self.text.parse::<Input>()
    }
}

#[derive(Debug, Error)]
enum SpiritCliError {
    #[error("component argument error: {0}")]
    Argument(#[from] ArgumentError),

    #[error("spirit requires exactly one inline NOTA/DOTOS input object")]
    InlineNotaRequired,

    #[error("invalid NOTA input: {0}")]
    NotaDecode(#[from] NotaDecodeError),

    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    #[cfg(feature = "testing-trace")]
    #[error("trace error: {0}")]
    Trace(#[from] TraceError),
}
