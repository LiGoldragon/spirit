//! Daemon-side runtime wrapper for the contract-owned configuration.
//!
//! The archived daemon configuration type lives in `signal-spirit`; this
//! wrapper holds the imported wire object plus daemon-local `PathBuf` views and
//! implements the `triad_runtime::BindingSurface` trait the emitted daemon spine
//! reads to bind listeners and open the store. No NOTA is linked here.

use std::{
    fs,
    path::{Path, PathBuf},
};

use signal_spirit::{ConfigurationPath, SpiritDaemonConfiguration};
use thiserror::Error;
use triad_runtime::BindingSurface;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Configuration {
    raw: SpiritDaemonConfiguration,
    socket_path: PathBuf,
    meta_socket_path: Option<PathBuf>,
    database_path: PathBuf,
    trace_socket_path: Option<PathBuf>,
}

impl Configuration {
    pub fn new(socket_path: impl AsRef<Path>, database_path: impl AsRef<Path>) -> Self {
        Self::from_raw(SpiritDaemonConfiguration::new(
            ConfigurationPath::new(socket_path.as_ref().to_string_lossy().into_owned()),
            ConfigurationPath::new(database_path.as_ref().to_string_lossy().into_owned()),
        ))
    }

    pub fn new_with_trace(
        socket_path: impl AsRef<Path>,
        database_path: impl AsRef<Path>,
        trace_socket_path: impl AsRef<Path>,
    ) -> Self {
        Self::new(socket_path, database_path).with_trace_socket_path(trace_socket_path)
    }

    pub fn with_meta_socket_path(self, meta_socket_path: impl AsRef<Path>) -> Self {
        Self::from_raw(self.raw.with_meta_socket_path(ConfigurationPath::new(
            meta_socket_path.as_ref().to_string_lossy().into_owned(),
        )))
    }

    pub fn with_trace_socket_path(self, trace_socket_path: impl AsRef<Path>) -> Self {
        Self::from_raw(self.raw.with_trace_socket_path(ConfigurationPath::new(
            trace_socket_path.as_ref().to_string_lossy().into_owned(),
        )))
    }

    pub fn from_raw(raw: SpiritDaemonConfiguration) -> Self {
        Self {
            socket_path: PathBuf::from(raw.socket_path()),
            meta_socket_path: raw.meta_socket_path().map(PathBuf::from),
            database_path: PathBuf::from(raw.database_path()),
            trace_socket_path: raw.trace_socket_path().map(PathBuf::from),
            raw,
        }
    }

    pub fn raw(&self) -> &SpiritDaemonConfiguration {
        &self.raw
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn meta_socket_path(&self) -> Option<&Path> {
        self.meta_socket_path.as_deref()
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn trace_socket_path(&self) -> Option<&Path> {
        self.trace_socket_path.as_deref()
    }

    pub fn from_binary_path(path: impl AsRef<Path>) -> Result<Self, ConfigurationError> {
        let bytes = fs::read(path).map_err(ConfigurationError::Read)?;
        Self::from_binary_bytes(&bytes)
    }

    pub fn from_binary_bytes(bytes: &[u8]) -> Result<Self, ConfigurationError> {
        SpiritDaemonConfiguration::from_rkyv_bytes(bytes)
            .map(Self::from_raw)
            .map_err(|_| ConfigurationError::ArchiveDecode)
    }

    pub fn to_binary_bytes(&self) -> Result<Vec<u8>, ConfigurationError> {
        self.raw
            .to_rkyv_bytes()
            .map_err(|_| ConfigurationError::ArchiveEncode)
    }

    pub fn write_binary_file(&self, path: impl AsRef<Path>) -> Result<(), ConfigurationError> {
        fs::write(path, self.to_binary_bytes()?).map_err(ConfigurationError::Write)
    }
}

impl BindingSurface for Configuration {
    fn socket_path(&self) -> &Path {
        Configuration::socket_path(self)
    }

    fn meta_socket_path(&self) -> Option<&Path> {
        Configuration::meta_socket_path(self)
    }

    fn database_path(&self) -> &Path {
        Configuration::database_path(self)
    }

    fn trace_socket_path(&self) -> Option<&Path> {
        Configuration::trace_socket_path(self)
    }
}

#[derive(Debug, Error)]
pub enum ConfigurationError {
    #[error("failed to read binary configuration: {0}")]
    Read(std::io::Error),

    #[error("failed to write binary configuration: {0}")]
    Write(std::io::Error),

    #[error("failed to encode binary configuration")]
    ArchiveEncode,

    #[error("failed to decode binary configuration")]
    ArchiveDecode,
}
