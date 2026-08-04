use std::env;

use nota::NotaDecodeError;
use spirit::{MetaSignalTransport, MetaTransportError, schema::meta_signal::Input as MetaInput};
use thiserror::Error;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand};

fn main() {
    if let Err(error) = MetaSpiritCli::from_environment().run() {
        eprintln!("meta-spirit: {error}");
        std::process::exit(1);
    }
}

/// The owner-only meta-signal client: the privileged sibling of `spirit`. It
/// speaks the meta wire vocabulary (`Configure`, `Import`) over the daemon's
/// owner-only meta socket, mirroring `spirit.rs` but typed on `MetaInput`.
struct MetaSpiritCli {
    command: ComponentCommand,
}

impl MetaSpiritCli {
    fn from_environment() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
        }
    }

    fn run(&self) -> Result<(), MetaSpiritCliError> {
        let source = self.source()?;
        let input = source.parse_input()?;
        let socket_path = env::var("SPIRIT_META_SOCKET")
            .unwrap_or_else(|_| String::from("/tmp/meta-spirit.sock"));
        let mut transport = MetaSignalTransport::connect(socket_path)?;
        let (_route, output) = transport.exchange(&input)?;
        println!("{output}");
        Ok(())
    }

    fn source(&self) -> Result<MetaSpiritInputSource, MetaSpiritCliError> {
        match self.command.nota_argument()? {
            ComponentArgument::InlineNota(argument) => {
                Ok(MetaSpiritInputSource::new(argument.into_string()))
            }
            ComponentArgument::NotaFile(_) | ComponentArgument::SignalFile(_) => {
                Err(MetaSpiritCliError::InlineNotaRequired)
            }
        }
    }
}

struct MetaSpiritInputSource {
    text: String,
}

impl MetaSpiritInputSource {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn parse_input(&self) -> Result<MetaInput, NotaDecodeError> {
        self.text.parse::<MetaInput>()
    }
}

#[derive(Debug, Error)]
enum MetaSpiritCliError {
    #[error("component argument error: {0}")]
    Argument(#[from] ArgumentError),

    #[error("meta-spirit requires exactly one inline NOTA/DOTOS input object")]
    InlineNotaRequired,

    #[error("invalid NOTA input: {0}")]
    NotaDecode(#[from] NotaDecodeError),

    #[error("meta transport error: {0}")]
    Transport(#[from] MetaTransportError),
}
