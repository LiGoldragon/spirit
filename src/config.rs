use std::{fmt, fs, path::Path};

/// Daemon configuration loaded from a binary rkyv file.
///
/// The daemon intentionally does not decode NOTA at startup. Text-facing
/// launchers or tests can produce this binary object, but the daemon itself
/// only receives the binary configuration path.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Configuration {
    socket_path: ConfigurationPath,
    database_path: ConfigurationPath,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ConfigurationPath(String);

impl ConfigurationPath {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self(path.as_ref().to_string_lossy().into_owned())
    }

    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl Configuration {
    pub fn new(socket_path: impl AsRef<Path>, database_path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: ConfigurationPath::new(socket_path),
            database_path: ConfigurationPath::new(database_path),
        }
    }

    pub fn socket_path(&self) -> &Path {
        self.socket_path.as_path()
    }

    pub fn database_path(&self) -> &Path {
        self.database_path.as_path()
    }

    pub fn from_single_argument(argument: &str) -> Result<Self, ConfigurationError> {
        Self::from_binary_file(Path::new(argument))
    }

    pub fn from_binary_file(path: impl AsRef<Path>) -> Result<Self, ConfigurationError> {
        let bytes = fs::read(path).map_err(ConfigurationError::Read)?;
        Self::from_binary_bytes(&bytes)
    }

    pub fn from_binary_bytes(bytes: &[u8]) -> Result<Self, ConfigurationError> {
        rkyv::from_bytes::<Self, rkyv::rancor::Error>(bytes)
            .map_err(|_| ConfigurationError::ArchiveDecode)
    }

    pub fn to_binary_bytes(&self) -> Result<Vec<u8>, ConfigurationError> {
        rkyv::to_bytes::<rkyv::rancor::Error>(self)
            .map(|bytes| bytes.to_vec())
            .map_err(|_| ConfigurationError::ArchiveEncode)
    }

    pub fn write_binary_file(&self, path: impl AsRef<Path>) -> Result<(), ConfigurationError> {
        fs::write(path, self.to_binary_bytes()?).map_err(ConfigurationError::Write)
    }
}

#[derive(Debug)]
pub enum ConfigurationError {
    Read(std::io::Error),
    Write(std::io::Error),
    ArchiveEncode,
    ArchiveDecode,
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "failed to read binary configuration: {error}"),
            Self::Write(error) => {
                write!(formatter, "failed to write binary configuration: {error}")
            }
            Self::ArchiveEncode => formatter.write_str("failed to encode binary configuration"),
            Self::ArchiveDecode => formatter.write_str("failed to decode binary configuration"),
        }
    }
}

impl std::error::Error for ConfigurationError {}
