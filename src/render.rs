use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use nota::{NotaDecode, NotaDecodeError, NotaEncode, NotaSource};
use thiserror::Error;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand};

use crate::{
    SignalTransport, TransportError,
    schema::signal::{
        Certainty, CertaintySelection, DomainMatch, ImportanceSelection, Input, KeywordMatch,
        Magnitude, ObservedRecords, Output, Privacy, PrivacySelection, Query, RecordSet, Referent,
        ReferentSelection, Referents, SelectedKind, TextMatch,
    },
};

const OUTPUT_FILE_NAME: &str = "spirit.nota";

pub struct RenderCli {
    command: ComponentCommand,
}

pub struct RenderInputSource {
    text: String,
}

pub struct RenderClient {
    socket_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode)]
pub enum RenderInput {
    Render(RenderRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode)]
pub struct RenderRequest {
    referents: RenderReferents,
    output_directory: Option<RenderDirectory>,
}

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct RenderReferents(Vec<RenderReferent>);

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct RenderReferent(String);

#[derive(Debug, Clone, PartialEq, Eq, NotaDecode, NotaEncode)]
pub struct RenderDirectory(String);

pub struct RenderedIntent {
    request: RenderRequest,
    marker: Output,
    records: Output,
    timestamp: RenderTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderTimestamp {
    unix_seconds: u64,
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("component argument error: {0}")]
    Argument(#[from] ArgumentError),

    #[error("failed to read NOTA file {}: {source}", path.display())]
    ReadNotaFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("signal archive file arguments are not supported by spirit-render: {}", path.display())]
    UnsupportedSignalFile { path: PathBuf },

    #[error("invalid NOTA input: {0}")]
    NotaDecode(#[from] NotaDecodeError),

    #[error("render request must name at least one referent")]
    EmptyReferents,

    #[error("system clock is before unix epoch")]
    ClockBeforeUnixEpoch,

    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    #[error("expected {expected} from Spirit daemon, got {actual}")]
    UnexpectedOutput {
        expected: &'static str,
        actual: String,
    },

    #[error("failed to create output directory {}: {source}", path.display())]
    CreateOutputDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write {}: {source}", path.display())]
    WriteOutput {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl RenderCli {
    pub fn from_environment() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
        }
    }

    pub fn run(&self) -> Result<(), RenderError> {
        let source = self.source()?;
        let request = source.parse_request()?;
        let socket_path =
            env::var("SPIRIT_SOCKET").unwrap_or_else(|_| String::from("/tmp/spirit.sock"));
        let client = RenderClient::new(socket_path);
        let rendered = client.render(request)?;
        let output_path = rendered.write()?;
        println!("{}", output_path.display());
        Ok(())
    }

    fn source(&self) -> Result<RenderInputSource, RenderError> {
        match self.command.nota_argument()? {
            ComponentArgument::InlineNota(argument) => {
                Ok(RenderInputSource::new(argument.into_string()))
            }
            ComponentArgument::NotaFile(file) => {
                let path = file.into_path();
                fs::read_to_string(&path)
                    .map(RenderInputSource::new)
                    .map_err(|source| RenderError::ReadNotaFile { path, source })
            }
            ComponentArgument::SignalFile(file) => Err(RenderError::UnsupportedSignalFile {
                path: file.into_path(),
            }),
        }
    }
}

impl RenderInputSource {
    pub fn new(text: String) -> Self {
        Self { text }
    }

    pub fn parse_request(&self) -> Result<RenderRequest, RenderError> {
        NotaSource::new(&self.text)
            .parse::<RenderInput>()?
            .into_request()
    }
}

impl RenderInput {
    pub fn into_request(self) -> Result<RenderRequest, RenderError> {
        match self {
            Self::Render(request) => request.validate(),
        }
    }
}

impl RenderRequest {
    pub fn new(referents: RenderReferents, output_directory: Option<RenderDirectory>) -> Self {
        Self {
            referents,
            output_directory,
        }
    }

    pub fn output_path(&self) -> Result<PathBuf, RenderError> {
        Ok(self.output_directory()?.join(OUTPUT_FILE_NAME))
    }

    fn validate(self) -> Result<Self, RenderError> {
        if self.referents.is_empty() {
            Err(RenderError::EmptyReferents)
        } else {
            Ok(self)
        }
    }

    fn output_directory(&self) -> Result<PathBuf, RenderError> {
        match &self.output_directory {
            Some(directory) => Ok(directory.path_buf()),
            None => env::current_dir().map_err(|source| RenderError::CreateOutputDirectory {
                path: PathBuf::from("."),
                source,
            }),
        }
    }

    fn query(&self) -> Query {
        Query {
            domain_match: DomainMatch::Any,
            keyword_match: KeywordMatch::Any,
            text_match: TextMatch::Any,
            referent_selection: self.referents.referent_selection(),
            selected_kind: SelectedKind::new(None),
            privacy_selection: PrivacySelection::exact(Privacy::new(Magnitude::Zero)),
            certainty_selection: CertaintySelection::at_least_certainty(Certainty::new(
                Magnitude::Minimum,
            )),
            importance_selection: ImportanceSelection::Any,
        }
    }
}

impl RenderReferents {
    pub fn new(referents: Vec<RenderReferent>) -> Self {
        Self(referents)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn referent_selection(&self) -> ReferentSelection {
        ReferentSelection::any_referent(Referents::new(
            self.0
                .iter()
                .cloned()
                .map(RenderReferent::into_signal_referent)
                .collect(),
        ))
    }
}

impl RenderReferent {
    pub fn new(referent: impl Into<String>) -> Self {
        Self(referent.into())
    }

    fn into_signal_referent(self) -> Referent {
        Referent::new(self.0)
    }
}

impl RenderDirectory {
    pub fn new(directory: impl Into<String>) -> Self {
        Self(directory.into())
    }

    fn path_buf(&self) -> PathBuf {
        PathBuf::from(&self.0)
    }
}

impl RenderClient {
    pub fn new(socket_path: String) -> Self {
        Self { socket_path }
    }

    pub fn render(&self, request: RenderRequest) -> Result<RenderedIntent, RenderError> {
        let timestamp = RenderTimestamp::now()?;
        let marker = self.marker()?;
        let records = self.records(&request)?;
        Ok(RenderedIntent::new(request, marker, records, timestamp))
    }

    fn marker(&self) -> Result<Output, RenderError> {
        let mut transport = SignalTransport::connect(&self.socket_path)?;
        let (_route, output) = transport.exchange(&Input::Marker)?;
        match output {
            Output::MarkerReported(_) => Ok(output),
            other => Err(RenderError::UnexpectedOutput {
                expected: "MarkerReported",
                actual: other.to_string(),
            }),
        }
    }

    fn records(&self, request: &RenderRequest) -> Result<Output, RenderError> {
        let mut transport = SignalTransport::connect(&self.socket_path)?;
        let (_route, output) = transport.exchange(&Input::observe(request.query()))?;
        match output {
            Output::RecordsObserved(_) => Ok(output),
            Output::RecordsStashed(stashed) => Ok(Output::records_observed(
                stashed.into_payload().observed_records,
            )),
            Output::Error(error) if error.payload().payload().payload() == "no matching record" => {
                Ok(Output::records_observed(ObservedRecords::new(
                    RecordSet::new(Vec::new()),
                )))
            }
            other => Err(RenderError::UnexpectedOutput {
                expected: "RecordsObserved or RecordsStashed",
                actual: other.to_string(),
            }),
        }
    }
}

impl RenderedIntent {
    pub fn new(
        request: RenderRequest,
        marker: Output,
        records: Output,
        timestamp: RenderTimestamp,
    ) -> Self {
        Self {
            request,
            marker,
            records,
            timestamp,
        }
    }

    pub fn write(&self) -> Result<PathBuf, RenderError> {
        let output_path = self.request.output_path()?;
        if let Some(parent) = output_path.parent() {
            self.create_output_directory(parent)?;
        }
        fs::write(&output_path, self.document()).map_err(|source| RenderError::WriteOutput {
            path: output_path.clone(),
            source,
        })?;
        Ok(output_path)
    }

    pub fn document(&self) -> String {
        format!(
            ";; @generated by spirit-render\n\
             ;; generator-version: {}\n\
             ;; generated-unix-timestamp-seconds: {}\n\
             ;; spirit-marker: {}\n\
             ;; privacy-filter: Zero\n\
             ;; certainty-filter: AtLeast Minimum\n\
             ;; criome-auth-reference: None\n\
             {}\n",
            env!("CARGO_PKG_VERSION"),
            self.timestamp.unix_seconds(),
            self.marker,
            self.records,
        )
    }

    fn create_output_directory(&self, directory: &Path) -> Result<(), RenderError> {
        fs::create_dir_all(directory).map_err(|source| RenderError::CreateOutputDirectory {
            path: directory.to_path_buf(),
            source,
        })
    }
}

impl RenderTimestamp {
    pub fn new(unix_seconds: u64) -> Self {
        Self { unix_seconds }
    }

    pub fn now() -> Result<Self, RenderError> {
        let unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| RenderError::ClockBeforeUnixEpoch)?
            .as_secs();
        Ok(Self::new(unix_seconds))
    }

    pub fn unix_seconds(&self) -> u64 {
        self.unix_seconds
    }
}
