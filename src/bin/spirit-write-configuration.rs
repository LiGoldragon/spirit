use std::{
    fs,
    path::{Path, PathBuf},
};

use nota_next::{NotaDecode, NotaDecodeError, NotaEncode, NotaSource};
use signal_spirit::{
    AuthorizationMode, ConfigurationPath, SpiritDaemonConfiguration,
    SpiritDaemonConfigurationArchiveError, SpiritGuardianAgentConfiguration,
    SpiritGuardianMaximumOutputTokens, SpiritGuardianModelName, SpiritGuardianProviderName,
    SpiritGuardianTimeoutMilliseconds,
};
use thiserror::Error;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand};

fn main() {
    if let Err(error) = ConfigurationWriterCli::from_environment().run() {
        eprintln!("spirit-write-configuration: {error}");
        std::process::exit(1);
    }
}

struct ConfigurationWriterCli {
    command: ComponentCommand,
}

struct ConfigurationWriterInputSource {
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode)]
struct ConfigurationWriteRequest {
    socket_path: ConfigurationWriterPath,
    meta_socket_path: Option<ConfigurationWriterPath>,
    database_path: ConfigurationWriterPath,
    trace_socket_path: Option<ConfigurationWriterPath>,
    authorization_mode: AuthorizationMode,
    guardian_agent_configuration: Option<ConfigurationWriterGuardianAgent>,
    output_path: ConfigurationWriterPath,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode)]
enum ConfigurationWriterInput {
    ConfigurationWriteRequest(ConfigurationWriteRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
struct ConfigurationWriterPath(String);

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
struct ConfigurationWriterProviderName(String);

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
struct ConfigurationWriterModelName(String);

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
struct ConfigurationWriterTimeoutMilliseconds(u64);

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
struct ConfigurationWriterMaximumOutputTokens(u64);

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
struct ConfigurationWriterGuardianAgent {
    agent_socket_path: ConfigurationWriterPath,
    provider_name: Option<ConfigurationWriterProviderName>,
    model_name: Option<ConfigurationWriterModelName>,
    timeout_milliseconds: ConfigurationWriterTimeoutMilliseconds,
    maximum_output_tokens: Option<ConfigurationWriterMaximumOutputTokens>,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaEncode)]
enum ConfigurationWriteOutput {
    ConfigurationWritten(ConfigurationWriterPath),
}

impl ConfigurationWriterCli {
    fn from_environment() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
        }
    }

    fn run(&self) -> Result<(), ConfigurationWriterCliError> {
        let source = self.source()?;
        let request = source.parse_request()?;
        let output = request.write()?;
        println!("{}", output.to_nota());
        Ok(())
    }

    fn source(&self) -> Result<ConfigurationWriterInputSource, ConfigurationWriterCliError> {
        match self.command.nota_argument()? {
            ComponentArgument::InlineNota(argument) => {
                Ok(ConfigurationWriterInputSource::new(argument.into_string()))
            }
            ComponentArgument::NotaFile(file) => {
                let path = file.into_path();
                fs::read_to_string(&path)
                    .map(ConfigurationWriterInputSource::new)
                    .map_err(|source| ConfigurationWriterCliError::ReadNotaFile { path, source })
            }
            ComponentArgument::SignalFile(file) => {
                Err(ConfigurationWriterCliError::UnsupportedSignalFile {
                    path: file.into_path(),
                })
            }
        }
    }
}

impl ConfigurationWriterInputSource {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn parse_request(&self) -> Result<ConfigurationWriteRequest, NotaDecodeError> {
        NotaSource::new(&self.text)
            .parse::<ConfigurationWriterInput>()
            .map(ConfigurationWriterInput::into_request)
    }
}

impl ConfigurationWriterInput {
    fn into_request(self) -> ConfigurationWriteRequest {
        match self {
            Self::ConfigurationWriteRequest(request) => request,
        }
    }
}

impl ConfigurationWriteRequest {
    fn write(self) -> Result<ConfigurationWriteOutput, ConfigurationWriterCliError> {
        let output_path = self.output_path.clone();
        let configuration = self.configuration();
        fs::write(output_path.as_path(), configuration.to_rkyv_bytes()?).map_err(|source| {
            ConfigurationWriterCliError::WriteArchive {
                path: output_path.path_buf(),
                source,
            }
        })?;
        Ok(ConfigurationWriteOutput::ConfigurationWritten(output_path))
    }

    fn configuration(self) -> SpiritDaemonConfiguration {
        let mut configuration = SpiritDaemonConfiguration::new(
            self.socket_path.into_configuration_path(),
            self.database_path.into_configuration_path(),
        );
        if let Some(meta_socket_path) = self.meta_socket_path {
            configuration =
                configuration.with_meta_socket_path(meta_socket_path.into_configuration_path());
        }
        if let Some(trace_socket_path) = self.trace_socket_path {
            configuration =
                configuration.with_trace_socket_path(trace_socket_path.into_configuration_path());
        }
        configuration = configuration.with_authorization_mode(self.authorization_mode);
        if let Some(guardian_agent_configuration) = self.guardian_agent_configuration {
            configuration = configuration.with_guardian_agent_configuration(
                guardian_agent_configuration.into_guardian_agent_configuration(),
            );
        }
        configuration
    }
}

impl ConfigurationWriterPath {
    fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    fn path_buf(&self) -> PathBuf {
        self.as_path().to_path_buf()
    }

    fn into_configuration_path(self) -> ConfigurationPath {
        ConfigurationPath::new(self.0)
    }
}

impl ConfigurationWriterGuardianAgent {
    fn into_guardian_agent_configuration(self) -> SpiritGuardianAgentConfiguration {
        SpiritGuardianAgentConfiguration::new(
            self.agent_socket_path.into_configuration_path(),
            self.provider_name
                .map(ConfigurationWriterProviderName::into_provider_name),
            self.model_name
                .map(ConfigurationWriterModelName::into_model_name),
            self.timeout_milliseconds.into_timeout_milliseconds(),
            self.maximum_output_tokens
                .map(ConfigurationWriterMaximumOutputTokens::into_maximum_output_tokens),
        )
    }
}

impl ConfigurationWriterProviderName {
    fn into_provider_name(self) -> SpiritGuardianProviderName {
        SpiritGuardianProviderName::new(self.0)
    }
}

impl ConfigurationWriterModelName {
    fn into_model_name(self) -> SpiritGuardianModelName {
        SpiritGuardianModelName::new(self.0)
    }
}

impl ConfigurationWriterTimeoutMilliseconds {
    fn into_timeout_milliseconds(self) -> SpiritGuardianTimeoutMilliseconds {
        SpiritGuardianTimeoutMilliseconds::new(self.0)
    }
}

impl ConfigurationWriterMaximumOutputTokens {
    fn into_maximum_output_tokens(self) -> SpiritGuardianMaximumOutputTokens {
        SpiritGuardianMaximumOutputTokens::new(self.0)
    }
}

#[derive(Debug, Error)]
enum ConfigurationWriterCliError {
    #[error(transparent)]
    Argument(#[from] ArgumentError),
    #[error("read NOTA file {}: {source}", path.display())]
    ReadNotaFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(
        "signal-encoded configuration writer requests are not implemented yet for {}",
        path.display()
    )]
    UnsupportedSignalFile { path: PathBuf },
    #[error(transparent)]
    Decode(#[from] NotaDecodeError),
    #[error(transparent)]
    Archive(#[from] SpiritDaemonConfigurationArchiveError),
    #[error("write binary configuration archive {}: {source}", path.display())]
    WriteArchive {
        path: PathBuf,
        source: std::io::Error,
    },
}
