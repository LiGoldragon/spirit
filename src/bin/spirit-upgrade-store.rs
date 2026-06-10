use std::{fs, path::PathBuf};

use nota_next::{NotaDecodeError, NotaEncode, NotaSource};
use spirit::{ProductionMigrationError, SpiritStoreUpgrade, SpiritStoreUpgradeRequest};
use thiserror::Error;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand};

fn main() {
    if let Err(error) = SpiritStoreUpgradeCli::from_environment().run() {
        eprintln!("spirit-upgrade-store: {error}");
        std::process::exit(1);
    }
}

struct SpiritStoreUpgradeCli {
    command: ComponentCommand,
}

struct SpiritStoreUpgradeInputSource {
    text: String,
}

impl SpiritStoreUpgradeCli {
    fn from_environment() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
        }
    }

    fn run(&self) -> Result<(), SpiritStoreUpgradeCliError> {
        let source = self.source()?;
        let request = source.parse_request()?;
        let output = SpiritStoreUpgrade::new(request).run()?;
        println!("{}", output.to_nota());
        Ok(())
    }

    fn source(&self) -> Result<SpiritStoreUpgradeInputSource, SpiritStoreUpgradeCliError> {
        match self.command.nota_argument()? {
            ComponentArgument::InlineNota(argument) => {
                Ok(SpiritStoreUpgradeInputSource::new(argument.into_string()))
            }
            ComponentArgument::NotaFile(file) => {
                let path = file.into_path();
                fs::read_to_string(&path)
                    .map(SpiritStoreUpgradeInputSource::new)
                    .map_err(|source| SpiritStoreUpgradeCliError::ReadNotaFile { path, source })
            }
            ComponentArgument::SignalFile(file) => {
                Err(SpiritStoreUpgradeCliError::UnsupportedSignalFile {
                    path: file.into_path(),
                })
            }
        }
    }
}

impl SpiritStoreUpgradeInputSource {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn parse_request(&self) -> Result<SpiritStoreUpgradeRequest, NotaDecodeError> {
        NotaSource::new(&self.text).parse()
    }
}

#[derive(Debug, Error)]
enum SpiritStoreUpgradeCliError {
    #[error(transparent)]
    Argument(#[from] ArgumentError),
    #[error("read NOTA file {}: {source}", path.display())]
    ReadNotaFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(
        "signal-encoded store upgrade requests are not implemented yet for {}",
        path.display()
    )]
    UnsupportedSignalFile { path: PathBuf },
    #[error(transparent)]
    Decode(#[from] NotaDecodeError),
    #[error(transparent)]
    Migration(#[from] ProductionMigrationError),
}
