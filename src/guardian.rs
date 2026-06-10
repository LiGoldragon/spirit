use std::{
    io::Write,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use nota_next::NotaSource;
use signal_agent::{
    ChatMessage, ChatTranscript, CompletionText, Input as AgentInput, MaximumOutputTokens,
    ModelName, Output as AgentOutput, OutputMode, Prompt, PromptOptions, ProviderName, SystemText,
};
use signal_spirit::SpiritGuardianAgentConfiguration;
use thiserror::Error;
use triad_runtime::{FrameBody, LengthPrefixedCodec};

use crate::schema::{
    nexus::{GuardianVerdict, Reject},
    signal::{
        DatabaseMarker, Entry, Explanation, GuardianRejection, GuardianRejectionReason, RecordSet,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentGuardianConfiguration {
    socket_path: PathBuf,
    provider_name: Option<String>,
    model_name: Option<String>,
    timeout: Duration,
    maximum_output_tokens: u64,
}

#[derive(Clone, Debug)]
pub struct AgentGuardian {
    configuration: AgentGuardianConfiguration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentGuardianRejection {
    reason: GuardianRejectionReason,
    records: RecordSet,
    explanation: Explanation,
    database_marker: DatabaseMarker,
}

#[derive(Debug, Error)]
pub enum AgentGuardianError {
    #[error("guardian agent socket unavailable: {0}")]
    Socket(std::io::Error),

    #[error("guardian agent frame failed: {0}")]
    Frame(String),

    #[error("guardian agent rejected the call: {0}")]
    AgentRejected(String),

    #[error("guardian agent returned malformed verdict: {0}")]
    Malformed(String),
}

impl AgentGuardianConfiguration {
    pub fn from_contract(configuration: &SpiritGuardianAgentConfiguration) -> Self {
        Self {
            socket_path: PathBuf::from(configuration.agent_socket_path()),
            provider_name: configuration.provider_name().map(ToOwned::to_owned),
            model_name: configuration.model_name().map(ToOwned::to_owned),
            timeout: Duration::from_millis(configuration.timeout_milliseconds()),
            maximum_output_tokens: configuration.maximum_output_tokens(),
        }
    }

    pub fn new(
        socket_path: impl Into<PathBuf>,
        provider_name: Option<String>,
        model_name: Option<String>,
        timeout: Duration,
        maximum_output_tokens: u64,
    ) -> Self {
        Self {
            socket_path: socket_path.into(),
            provider_name,
            model_name,
            timeout,
            maximum_output_tokens,
        }
    }
}

impl AgentGuardian {
    pub fn new(configuration: AgentGuardianConfiguration) -> Self {
        Self { configuration }
    }

    pub fn guard_proposal(
        &self,
        proposal: &Entry,
        records: RecordSet,
        database_marker: DatabaseMarker,
    ) -> Option<AgentGuardianRejection> {
        match self.call_guardian(proposal, &records) {
            Ok(GuardianVerdict::Accept) => None,
            Ok(GuardianVerdict::Reject(rejection)) => Some(AgentGuardianRejection::from_reject(
                rejection,
                records,
                database_marker,
            )),
            Err(error) => Some(AgentGuardianRejection::from_error(
                error,
                records,
                database_marker,
            )),
        }
    }

    fn call_guardian(
        &self,
        proposal: &Entry,
        records: &RecordSet,
    ) -> Result<GuardianVerdict, AgentGuardianError> {
        let output = self.call_agent(self.prompt(proposal, records)?)?;
        let AgentOutput::Completed(completion) = output else {
            return Err(AgentGuardianError::AgentRejected(format!("{output:?}")));
        };
        self.parse_verdict(&completion.text)
    }

    fn call_agent(&self, prompt: Prompt) -> Result<AgentOutput, AgentGuardianError> {
        let mut stream = UnixStream::connect(self.configuration.socket_path())
            .map_err(AgentGuardianError::Socket)?;
        stream
            .set_read_timeout(Some(self.configuration.timeout))
            .map_err(AgentGuardianError::Socket)?;
        stream
            .set_write_timeout(Some(self.configuration.timeout))
            .map_err(AgentGuardianError::Socket)?;
        let input = AgentInput::call(prompt);
        let codec = LengthPrefixedCodec::default();
        codec
            .write_body(
                &mut stream,
                &FrameBody::new(
                    input
                        .encode_signal_frame()
                        .map_err(|error| AgentGuardianError::Frame(error.to_string()))?,
                ),
            )
            .map_err(|error| AgentGuardianError::Frame(error.to_string()))?;
        stream.flush().map_err(AgentGuardianError::Socket)?;
        let reply = codec
            .read_body(&mut stream)
            .map_err(|error| AgentGuardianError::Frame(error.to_string()))?;
        AgentOutput::decode_signal_frame(&reply.into_bytes())
            .map(|(_route, output)| output)
            .map_err(|error| AgentGuardianError::Frame(error.to_string()))
    }

    fn prompt(&self, proposal: &Entry, records: &RecordSet) -> Result<Prompt, AgentGuardianError> {
        let accept = GuardianVerdict::Accept.to_nota();
        let reject = GuardianVerdict::reject(Reject {
            guardian_rejection_reason: GuardianRejectionReason::Contradiction,
            explanation: Explanation::new("explain the conflict"),
        })
        .to_nota();
        Ok(Prompt {
            system: Some(SystemText::new(format!(
                "You are Spirit's guardian. Judge whether a proposed forward intent arrow can enter a mutually-consistent intent store. Reply with exactly one NOTA value of the GuardianVerdict type. Accept form: {accept}. Reject form: {reject}. Reject on contradiction, compound intent, non-intent, unclear privacy, unclear category, or retrieval insufficiency."
            ))),
            transcript: ChatTranscript::new(vec![ChatMessage::user(format!(
                "Proposal:\n{}\n\nRelevant existing records:\n{}",
                proposal.to_nota(),
                records.to_nota()
            ))]),
            options: PromptOptions {
                model: self
                    .configuration
                    .model_name
                    .as_ref()
                    .map(|model| ModelName::new(model.clone())),
                provider: self
                    .configuration
                    .provider_name
                    .as_ref()
                    .map(|provider| ProviderName::new(provider.clone())),
                temperature_milli: None,
                maximum_output_tokens: Some(MaximumOutputTokens::new(
                    self.configuration.maximum_output_tokens,
                )),
                output_mode: OutputMode::Nota,
            },
        })
    }

    fn parse_verdict(
        &self,
        completion: &CompletionText,
    ) -> Result<GuardianVerdict, AgentGuardianError> {
        NotaSource::new(completion.payload())
            .parse::<GuardianVerdict>()
            .map_err(|error| AgentGuardianError::Malformed(error.to_string()))
    }
}

impl AgentGuardianConfiguration {
    fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl AgentGuardianRejection {
    fn from_reject(rejection: Reject, records: RecordSet, database_marker: DatabaseMarker) -> Self {
        Self {
            reason: rejection.guardian_rejection_reason,
            records,
            explanation: rejection.explanation,
            database_marker,
        }
    }

    fn from_error(
        error: AgentGuardianError,
        records: RecordSet,
        database_marker: DatabaseMarker,
    ) -> Self {
        let reason = match error {
            AgentGuardianError::Socket(_) | AgentGuardianError::Frame(_) => {
                GuardianRejectionReason::HarnessUnavailable
            }
            AgentGuardianError::AgentRejected(_) | AgentGuardianError::Malformed(_) => {
                GuardianRejectionReason::HarnessMalformed
            }
        };
        Self {
            reason,
            records,
            explanation: Explanation::new(error.to_string()),
            database_marker,
        }
    }

    pub fn into_guardian_rejection(self) -> GuardianRejection {
        GuardianRejection {
            guardian_rejection_reason: self.reason,
            record_set: self.records,
            explanation: self.explanation,
            database_marker: self.database_marker,
        }
    }
}
