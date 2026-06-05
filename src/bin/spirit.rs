use std::{env, fs, io::ErrorKind, os::unix::net::UnixStream, path::PathBuf};

use nota_next::{Delimiter, Document, NotaBlock, NotaDecodeError};
use spirit::{
    SignalTransport, TransportError,
    schema::signal::{Input, Output, Statement},
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
            ComponentArgument::NotaFile(file) => {
                let path = file.into_path();
                fs::read_to_string(&path)
                    .map(SpiritInputSource::new)
                    .map_err(|source| SpiritCliError::ReadNotaFile { path, source })
            }
            ComponentArgument::SignalFile(file) => {
                let path = file.into_path();
                fs::read_to_string(&path)
                    .map(SpiritInputSource::new)
                    .map_err(|source| SpiritCliError::ReadNotaFile { path, source })
            }
        }
    }
}

struct SpiritInputSource {
    text: String,
}

struct LegacyStateInput {
    statement: Statement,
}

impl SpiritInputSource {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn parse_input(&self) -> Result<Input, NotaDecodeError> {
        self.text.parse::<Input>().or_else(|error| {
            LegacyStateInput::from_source(&self.text)
                .map(LegacyStateInput::into_input)
                .ok_or(error)
        })
    }
}

impl LegacyStateInput {
    fn from_source(source: &str) -> Option<Self> {
        let document = Document::parse(source).ok()?;
        let [root] = document.root_objects() else {
            return None;
        };
        let [head, payload] = root.as_delimited(Delimiter::Parenthesis)? else {
            return None;
        };
        if head.demote_to_string()? != "State" {
            return None;
        }
        let [statement_text] = payload.as_delimited(Delimiter::Parenthesis)? else {
            return None;
        };
        let statement_text = NotaBlock::new(statement_text).parse_string().ok()?;
        Some(Self {
            statement: Statement::new(statement_text),
        })
    }

    fn into_input(self) -> Input {
        Input::state(self.statement)
    }
}

#[derive(Debug, Error)]
enum SpiritCliError {
    #[error("component argument error: {0}")]
    Argument(#[from] ArgumentError),

    #[error("failed to read NOTA file {}: {source}", path.display())]
    ReadNotaFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid NOTA input: {0}")]
    NotaDecode(#[from] NotaDecodeError),

    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    #[cfg(feature = "testing-trace")]
    #[error("trace error: {0}")]
    Trace(#[from] TraceError),
}
