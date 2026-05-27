use std::{fmt, fs, path::PathBuf};

use nota_next::{Block, Delimiter, Document};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Configuration {
    pub socket_path: PathBuf,
}

impl Configuration {
    pub fn from_single_argument(argument: &str) -> Result<Self, ConfigurationError> {
        let source = if argument.trim_start().starts_with(['(', '[', '{']) {
            argument.to_owned()
        } else {
            fs::read_to_string(argument).map_err(ConfigurationError::Read)?
        };
        let document = Document::parse(source).map_err(|error| {
            ConfigurationError::Nota(format!("failed to parse daemon configuration: {error}"))
        })?;
        if document.holds_root_objects() != 1 {
            return Err(ConfigurationError::ExpectedSingleRoot {
                found: document.holds_root_objects(),
            });
        }
        let root = document.root_object_at(0).expect("root count checked");
        let text = ConfigurationText::new(root).read()?;
        Ok(Self {
            socket_path: PathBuf::from(text),
        })
    }
}

#[derive(Debug)]
pub enum ConfigurationError {
    Read(std::io::Error),
    Nota(String),
    ExpectedSingleRoot { found: usize },
    ExpectedText,
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "failed to read configuration file: {error}"),
            Self::Nota(error) => formatter.write_str(error),
            Self::ExpectedSingleRoot { found } => {
                write!(formatter, "expected one configuration root, found {found}")
            }
            Self::ExpectedText => formatter.write_str("expected text configuration value"),
        }
    }
}

impl std::error::Error for ConfigurationError {}

struct ConfigurationText<'block> {
    block: &'block Block,
}

impl<'block> ConfigurationText<'block> {
    fn new(block: &'block Block) -> Self {
        Self { block }
    }

    fn read(&self) -> Result<String, ConfigurationError> {
        if let Some(text) = self.block.demote_to_string() {
            return Ok(text.to_owned());
        }
        match self.block {
            Block::Delimited {
                delimiter: Delimiter::SquareBracket,
                root_objects,
                ..
            } => root_objects
                .iter()
                .map(|block| Self::new(block).read())
                .collect::<Result<Vec<_>, _>>()
                .map(|parts| parts.join(" ")),
            _ => Err(ConfigurationError::ExpectedText),
        }
    }
}
