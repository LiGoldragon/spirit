use std::{env, fs, io::ErrorKind, os::unix::net::UnixStream, path::PathBuf};

use nota_next::{Delimiter, Document, NotaBlock, NotaDecode, NotaDecodeError};
use spirit::{
    SignalTransport, TransportError,
    schema::signal::{
        CertaintySelection, ImportanceSelection, Input, Kind, Output, PrivacySelection, Query,
        RemovalCandidateCollection, Statement, StatementText, TopicMatch,
    },
};
use thiserror::Error;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand, FrameError};

#[cfg(feature = "testing-trace")]
use spirit::{TraceClient, TraceError};
#[cfg(feature = "testing-trace")]
use std::time::Duration;

fn main() {
    if let Err(error) = SpiritCli::from_environment().run() {
        eprintln!("spirit: {error}");
        std::process::exit(1);
    }
}

struct SpiritCli {
    command: ComponentCommand,
}

impl SpiritCli {
    fn from_environment() -> Self {
        Self {
            command: ComponentCommand::from_environment(),
        }
    }

    fn run(&self) -> Result<(), SpiritCliError> {
        let source = self.source()?;
        let input = source.parse_input()?;
        let socket_path =
            env::var("SPIRIT_SOCKET").unwrap_or_else(|_| String::from("/tmp/spirit.sock"));
        #[cfg(feature = "testing-trace")]
        let trace_client =
            TraceClient::from_environment("SPIRIT_TRACE_SOCKET", Duration::from_millis(200))?;
        let opens_subscription = matches!(&input, Input::SubscribeIntent(_));
        let mut transport = SignalTransport::connect(socket_path)?;
        let (_route, output) = transport.exchange(&input)?;
        println!("{output}");
        if opens_subscription {
            self.print_subscription_events(&mut transport)?;
        }
        #[cfg(feature = "testing-trace")]
        trace_client.print_events(&mut std::io::stdout())?;
        Ok(())
    }

    fn print_subscription_events(
        &self,
        transport: &mut SignalTransport<UnixStream>,
    ) -> Result<(), SpiritCliError> {
        loop {
            match transport.read_subscription_event() {
                Ok(event) => println!("{}", Output::event(event)),
                Err(TransportError::Frame(FrameError::Io(error)))
                    if error.kind() == ErrorKind::UnexpectedEof =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn source(&self) -> Result<SpiritInputSource, SpiritCliError> {
        match self.command.nota_argument()? {
            ComponentArgument::InlineNota(argument) => {
                Ok(SpiritInputSource::new(argument.into_string()))
            }
            ComponentArgument::NotaFile(file) => {
                let path = file.into_path();
                fs::read_to_string(&path)
                    .map(SpiritInputSource::new)
                    .map_err(|source| SpiritCliError::ReadNotaFile { path, source })
            }
            ComponentArgument::SignalFile(file) => {
                let path = file.into_path();
                fs::read_to_string(&path)
                    .map(SpiritInputSource::new)
                    .map_err(|source| SpiritCliError::ReadNotaFile { path, source })
            }
        }
    }
}

struct SpiritInputSource {
    text: String,
}

struct LegacyStateInput {
    statement: Statement,
}

struct LegacyQueryInput {
    input: Input,
}

impl SpiritInputSource {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn parse_input(&self) -> Result<Input, NotaDecodeError> {
        match self.text.parse::<Input>() {
            Ok(input) => Ok(input),
            Err(error) => {
                if let Some(input) = LegacyStateInput::from_source(&self.text) {
                    return Ok(input.into_input());
                }
                if let Some(input) = LegacyQueryInput::from_source(&self.text)? {
                    return Ok(input.into_input());
                }
                Err(error)
            }
        }
    }
}

impl LegacyStateInput {
    fn from_source(source: &str) -> Option<Self> {
        let document = Document::parse(source).ok()?;
        let [root] = document.root_objects() else {
            return None;
        };
        let [head, payload] = root.as_delimited(Delimiter::Parenthesis)? else {
            return None;
        };
        if head.demote_to_string()? != "State" {
            return None;
        }
        let [statement_text] = payload.as_delimited(Delimiter::Parenthesis)? else {
            return None;
        };
        let statement_text = NotaBlock::new(statement_text).parse_string().ok()?;
        Some(Self {
            statement: Statement::new(StatementText::new(statement_text)),
        })
    }

    fn into_input(self) -> Input {
        Input::state(self.statement)
    }
}

impl LegacyQueryInput {
    fn from_source(source: &str) -> Result<Option<Self>, NotaDecodeError> {
        let Ok(document) = Document::parse(source) else {
            return Ok(None);
        };
        let [root] = document.root_objects() else {
            return Ok(None);
        };
        let Some([head, payload]) = root.as_delimited(Delimiter::Parenthesis) else {
            return Ok(None);
        };
        let Some(head) = head.demote_to_string() else {
            return Ok(None);
        };
        let input = match head {
            "Observe" => {
                let Some(query) = Self::observation_query(payload)? else {
                    return Ok(None);
                };
                Input::observe(query)
            }
            "Count" => {
                let Some(query) = Self::observation_query(payload)? else {
                    return Ok(None);
                };
                Input::count(query)
            }
            "SubscribeIntent" => {
                let Some(query) = Self::observation_query(payload)? else {
                    return Ok(None);
                };
                Input::subscribe_intent(query)
            }
            "CollectRemovalCandidates" => {
                let Some(query) = Self::removal_collection_query(payload)? else {
                    return Ok(None);
                };
                Input::collect_removal_candidates(RemovalCandidateCollection::new(query.into()))
            }
            _ => return Ok(None),
        };
        Ok(Some(Self { input }))
    }

    fn observation_query(payload: &nota_next::Block) -> Result<Option<Query>, NotaDecodeError> {
        Self::query_with_certainty(payload, CertaintySelection::default_observation_certainty())
    }

    fn removal_collection_query(
        payload: &nota_next::Block,
    ) -> Result<Option<Query>, NotaDecodeError> {
        if let Some(query) =
            Self::query_with_certainty(payload, CertaintySelection::removal_candidate_certainty())?
        {
            return Ok(Some(query));
        }
        let Some([query]) = payload.as_delimited(Delimiter::Parenthesis) else {
            return Ok(None);
        };
        Self::query_with_certainty(query, CertaintySelection::removal_candidate_certainty())
    }

    fn query_with_certainty(
        payload: &nota_next::Block,
        certainty_selection: CertaintySelection,
    ) -> Result<Option<Query>, NotaDecodeError> {
        let Some(fields) = payload.as_delimited(Delimiter::Parenthesis) else {
            return Ok(None);
        };
        let (topic_match, kind, privacy_selection, certainty_selection, importance_selection) =
            match fields {
                [topic_match, kind, privacy_selection] => (
                    topic_match,
                    kind,
                    privacy_selection,
                    certainty_selection,
                    ImportanceSelection::default_observation_importance(),
                ),
                [topic_match, kind, privacy_selection, certainty_selection] => (
                    topic_match,
                    kind,
                    privacy_selection,
                    CertaintySelection::from_nota_block(certainty_selection)?,
                    ImportanceSelection::default_observation_importance(),
                ),
                [
                    topic_match,
                    kind,
                    privacy_selection,
                    certainty_selection,
                    importance_selection,
                ] => (
                    topic_match,
                    kind,
                    privacy_selection,
                    CertaintySelection::from_nota_block(certainty_selection)?,
                    ImportanceSelection::from_nota_block(importance_selection)?,
                ),
                _ => return Ok(None),
            };
        Ok(Some(Query {
            topic_match: TopicMatch::from_nota_block(topic_match)?,
            kind: Option::<Kind>::from_nota_block(kind)?,
            privacy_selection: PrivacySelection::from_nota_block(privacy_selection)?,
            certainty_selection,
            importance_selection,
        }))
    }

    fn into_input(self) -> Input {
        self.input
    }
}

#[derive(Debug, Error)]
enum SpiritCliError {
    #[error("component argument error: {0}")]
    Argument(#[from] ArgumentError),

    #[error("failed to read NOTA file {}: {source}", path.display())]
    ReadNotaFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid NOTA input: {0}")]
    NotaDecode(#[from] NotaDecodeError),

    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    #[cfg(feature = "testing-trace")]
    #[error("trace error: {0}")]
    Trace(#[from] TraceError),
}
