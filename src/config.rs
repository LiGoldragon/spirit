//! Daemon-side behaviour for the schema-emitted `Configuration`.
//!
//! The `Configuration` data type is emitted from `schema/signal.schema` into
//! `crate::schema::signal` — the daemon owns its contract surface locally rather
//! than depending on the `signal-spirit` contract crate (see the crate docs).
//! This module attaches the runtime behaviour to that emitted type: constructors
//! for launchers and tests, path accessors, the binary rkyv read/write the
//! daemon decodes from its single startup argument, and the
//! `triad_runtime::BindingSurface` impl the emitted daemon spine reads to bind
//! listeners and open the store. No NOTA is linked here — the daemon stays
//! binary-only; the emitted type's `nota-text` surface is opt-in for text
//! clients only.

use std::{fs, path::Path};

use thiserror::Error;
use triad_runtime::BindingSurface;

use crate::schema::signal::Configuration;

impl Configuration {
    pub fn new(socket_path: impl AsRef<Path>, database_path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_string_lossy().into_owned(),
            meta_socket_path: None,
            database_path: database_path.as_ref().to_string_lossy().into_owned(),
            trace_socket_path: None,
        }
    }

    pub fn new_with_trace(
        socket_path: impl AsRef<Path>,
        database_path: impl AsRef<Path>,
        trace_socket_path: impl AsRef<Path>,
    ) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_string_lossy().into_owned(),
            meta_socket_path: None,
            database_path: database_path.as_ref().to_string_lossy().into_owned(),
            trace_socket_path: Some(trace_socket_path.as_ref().to_string_lossy().into_owned()),
        }
    }

    pub fn with_meta_socket_path(mut self, meta_socket_path: impl AsRef<Path>) -> Self {
        self.meta_socket_path = Some(meta_socket_path.as_ref().to_string_lossy().into_owned());
        self
    }

    pub fn socket_path(&self) -> &Path {
        Path::new(&self.socket_path)
    }

    pub fn meta_socket_path(&self) -> Option<&Path> {
        self.meta_socket_path.as_deref().map(Path::new)
    }

    pub fn database_path(&self) -> &Path {
        Path::new(&self.database_path)
    }

    pub fn trace_socket_path(&self) -> Option<&Path> {
        self.trace_socket_path.as_deref().map(Path::new)
    }

    pub fn from_binary_path(path: impl AsRef<Path>) -> Result<Self, ConfigurationError> {
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
