use std::{fs, path::PathBuf};

use nota_next::{NotaDecodeError, NotaEncode, NotaSource};
use spirit::{
    ProductionMigration, ProductionMigrationError, ProductionMigrationOutput,
    ProductionMigrationRequest,
};
use thiserror::Error;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand};

fn main() {
    if let Err(error) = ProductionMigrationCli::from_environment().run() {
        eprintln!("spirit-migrate-production: {error}");
        std::process::exit(1);
    }
}

struct ProductionMigrationCli {
    command: ComponentCommand,
}

struct ProductionMigrationInputSource {
    text: String,
}

impl ProductionMigrationCli {
    fn from_environment() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
        }
    }

    fn run(&self) -> Result<(), ProductionMigrationCliError> {
        let source = self.source()?;
        let request = source.parse_request()?;
        let completed = ProductionMigration::new(request).run()?;
        println!(
            "{}",
            ProductionMigrationOutput::completed(completed).to_nota()
        );
        Ok(())
    }

    fn source(&self) -> Result<ProductionMigrationInputSource, ProductionMigrationCliError> {
        match self.command.nota_argument()? {
            ComponentArgument::InlineNota(argument) => {
                Ok(ProductionMigrationInputSource::new(argument.into_string()))
            }
            ComponentArgument::NotaFile(file) => {
                let path = file.into_path();
                fs::read_to_string(&path)
                    .map(ProductionMigrationInputSource::new)
                    .map_err(|source| ProductionMigrationCliError::ReadNotaFile { path, source })
            }
            ComponentArgument::SignalFile(file) => {
                Err(ProductionMigrationCliError::UnsupportedSignalFile {
                    path: file.into_path(),
                })
            }
        }
    }
}

impl ProductionMigrationInputSource {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn parse_request(&self) -> Result<ProductionMigrationRequest, NotaDecodeError> {
        NotaSource::new(&self.text).parse()
    }
}

#[derive(Debug, Error)]
enum ProductionMigrationCliError {
    #[error("component argument error: {0}")]
    Argument(#[from] ArgumentError),

    #[error("failed to read NOTA file {}: {source}", path.display())]
    ReadNotaFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("signal file input is unsupported for production migration: {}", path.display())]
    UnsupportedSignalFile { path: PathBuf },

    #[error("invalid production migration request NOTA: {0}")]
    NotaDecode(#[from] NotaDecodeError),

    #[error("migration failed: {0}")]
    Migration(#[from] ProductionMigrationError),
}

impl From<ProductionMigrationCliError> for std::io::Error {
    fn from(error: ProductionMigrationCliError) -> Self {
        std::io::Error::other(error)
    }
}
