use std::{fs, path::PathBuf};

use nota::{NotaDecodeError, NotaEncode, NotaSource};
use spirit::{StoreMigration, StoreMigrationError, StoreMigrationRequest};
use thiserror::Error;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand};

fn main() {
    if let Err(error) = StoreMigrationCli::from_environment().run() {
        eprintln!("spirit-migrate-store: {error}");
        std::process::exit(1);
    }
}

struct StoreMigrationCli {
    command: ComponentCommand,
}

struct StoreMigrationInputSource {
    text: String,
}

impl StoreMigrationCli {
    fn from_environment() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
        }
    }

    fn run(&self) -> Result<(), StoreMigrationCliError> {
        let source = self.source()?;
        let request = source.parse_request()?;
        let output = StoreMigration::new(request).run()?;
        println!("{}", output.to_nota());
        Ok(())
    }

    fn source(&self) -> Result<StoreMigrationInputSource, StoreMigrationCliError> {
        match self.command.nota_argument()? {
            ComponentArgument::InlineNota(argument) => {
                Ok(StoreMigrationInputSource::new(argument.into_string()))
            }
            ComponentArgument::NotaFile(file) => {
                let path = file.into_path();
                fs::read_to_string(&path)
                    .map(StoreMigrationInputSource::new)
                    .map_err(|source| StoreMigrationCliError::ReadNotaFile { path, source })
            }
            ComponentArgument::SignalFile(file) => {
                Err(StoreMigrationCliError::UnsupportedSignalFile {
                    path: file.into_path(),
                })
            }
        }
    }
}

impl StoreMigrationInputSource {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn parse_request(&self) -> Result<StoreMigrationRequest, NotaDecodeError> {
        NotaSource::new(&self.text).parse()
    }
}

#[derive(Debug, Error)]
enum StoreMigrationCliError {
    #[error(transparent)]
    Argument(#[from] ArgumentError),

    #[error("read NOTA file {}: {source}", path.display())]
    ReadNotaFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error(
        "signal-encoded store migration requests are not implemented yet for {}",
        path.display()
    )]
    UnsupportedSignalFile { path: PathBuf },

    #[error(transparent)]
    Decode(#[from] NotaDecodeError),

    #[error(transparent)]
    Migration(#[from] StoreMigrationError),
}
