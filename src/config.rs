use std::{fmt, fs, path::PathBuf};

use nota_next::{Block, Delimiter, Document};

/// Daemon configuration, parsed from one positional NOTA record.
///
/// The single NOTA argument is a two-field positional record
/// `(<socket-path> <database-path>)`: the Unix socket the daemon serves,
/// and the durable SEMA `.sema` database file. The database path FILLS an
/// existing configuration position — there is no flag (the single-argument
/// rule holds). A bare text argument is still accepted as the socket path,
/// with the database defaulting beside it, so existing single-path
/// invocations keep working.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Configuration {
    pub socket_path: PathBuf,
    pub database_path: PathBuf,
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
        Self::from_root_block(root)
    }

    fn from_root_block(root: &Block) -> Result<Self, ConfigurationError> {
        if let Block::Delimited {
            delimiter: Delimiter::Parenthesis,
            root_objects,
            ..
        } = root
        {
            return match root_objects.as_slice() {
                [socket, database] => Ok(Self {
                    socket_path: PathBuf::from(ConfigurationText::new(socket).read()?),
                    database_path: PathBuf::from(ConfigurationText::new(database).read()?),
                }),
                other => Err(ConfigurationError::ExpectedFieldCount { found: other.len() }),
            };
        }
        let socket_path = PathBuf::from(ConfigurationText::new(root).read()?);
        let database_path = socket_path.with_extension("sema");
        Ok(Self {
            socket_path,
            database_path,
        })
    }
}

#[derive(Debug)]
pub enum ConfigurationError {
    Read(std::io::Error),
    Nota(String),
    ExpectedSingleRoot { found: usize },
    ExpectedFieldCount { found: usize },
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
            Self::ExpectedFieldCount { found } => write!(
                formatter,
                "expected a (socket database) configuration record, found {found} fields"
            ),
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
