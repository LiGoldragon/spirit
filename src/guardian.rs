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
    TemperatureMilli,
};
use signal_spirit::SpiritGuardianAgentConfiguration;
use thiserror::Error;
use triad_runtime::{FrameBody, LengthPrefixedCodec};

use crate::{
    guardian_journal::{GuardianDecision, GuardianOperation},
    schema::{
        nexus::{GuardianVerdict, Reject},
        signal::{
            DatabaseMarker, Explanation, GuardianRejection, GuardianRejectionReason, RecordSet,
        },
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentGuardianDecision {
    verdict: GuardianVerdict,
    records: RecordSet,
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

    pub(crate) fn guard(
        &self,
        operation: &GuardianOperation,
        records: RecordSet,
        database_marker: DatabaseMarker,
    ) -> AgentGuardianDecision {
        let verdict = self
            .call_guardian(operation, &records)
            .unwrap_or_else(|error| {
                GuardianVerdict::from_harness_rejection(
                    error.guardian_rejection_reason(),
                    Explanation::new(error.to_string()),
                )
            });
        AgentGuardianDecision::new(verdict, records, database_marker)
    }

    fn call_guardian(
        &self,
        operation: &GuardianOperation,
        records: &RecordSet,
    ) -> Result<GuardianVerdict, AgentGuardianError> {
        let output = self.call_agent(self.prompt(operation, records, None)?)?;
        let AgentOutput::Completed(completion) = output else {
            return Err(AgentGuardianError::AgentRejected(format!("{output:?}")));
        };
        match self.parse_verdict(&completion.text) {
            Ok(verdict) => Ok(verdict),
            Err(first_error) => {
                let retry = GuardianRetry::new(completion.text, first_error.to_string());
                let retry_output =
                    self.call_agent(self.prompt(operation, records, Some(&retry))?)?;
                let AgentOutput::Completed(retry_completion) = retry_output else {
                    return Err(AgentGuardianError::AgentRejected(format!(
                        "{retry_output:?}"
                    )));
                };
                self.parse_verdict(&retry_completion.text)
            }
        }
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

    fn prompt(
        &self,
        operation: &GuardianOperation,
        records: &RecordSet,
        retry: Option<&GuardianRetry>,
    ) -> Result<Prompt, AgentGuardianError> {
        let accept = GuardianVerdict::Accept.to_nota();
        let reject = GuardianVerdict::reject(Reject {
            guardian_rejection_reason: GuardianRejectionReason::Contradiction,
            explanation: Explanation::new("explain the conflict"),
        })
        .to_nota();
        let retry_text = retry
            .map(GuardianRetry::user_text)
            .unwrap_or_else(String::new);
        Ok(Prompt {
            system: Some(SystemText::new(format!(
                "You are Spirit's guardian. Judge whether one write operation may change the live intent store. The model checks every semantic admission issue: duplicates, contradictions, compound arrows, non-intent, unclear privacy, unclear domain, clarification damage, supersession damage, and retrieval insufficiency. Code only validates structure and applies the typed verdict. Reply with exactly one NOTA value of the GuardianVerdict type and no surrounding prose. Accept form: {accept}. Reject form: {reject}. Closed rejection reasons: Duplicate, Contradiction, Compound, NonIntent, UnclearPrivacy, UnclearDomain, ClarifyTramples, ClarifyLosesMeaning, SupersedeTargetMissing, RetrievalInsufficient, HarnessUnavailable, HarnessMalformed, HarnessTimedOut. Duplicate means the candidate already says the same forward arrow as a live record. Contradiction means it conflicts with a live arrow. Compound means it bundles multiple separable arrows. NonIntent means it is task chatter, status, or transient reaction rather than durable intent. UnclearPrivacy means the privacy level is unsafe or underspecified. UnclearDomain means the domain classification is missing or wrong enough to block admission. ClarifyTramples means a clarification overwrites the prior arrow instead of making it clearer. ClarifyLosesMeaning means a clarification drops material meaning. SupersedeTargetMissing means the replacement cannot be judged against the claimed target. RetrievalInsufficient means the supplied bundle is too thin to judge."
            ))),
            transcript: ChatTranscript::new(vec![ChatMessage::user(format!(
                "Operation: {}\n\nCandidate:\n{}\n\nRelevant existing records:\n{}{}",
                operation.name(),
                GuardianOperationPrompt::new(operation).to_text(),
                records.to_nota(),
                retry_text
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
                temperature_milli: Some(TemperatureMilli::new(0)),
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

impl AgentGuardianError {
    fn guardian_rejection_reason(&self) -> GuardianRejectionReason {
        match self {
            Self::Socket(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                GuardianRejectionReason::HarnessTimedOut
            }
            Self::Socket(_) | Self::Frame(_) => GuardianRejectionReason::HarnessUnavailable,
            Self::AgentRejected(_) | Self::Malformed(_) => {
                GuardianRejectionReason::HarnessMalformed
            }
        }
    }
}

impl AgentGuardianDecision {
    fn new(verdict: GuardianVerdict, records: RecordSet, database_marker: DatabaseMarker) -> Self {
        Self {
            verdict,
            records,
            database_marker,
        }
    }

    pub(crate) fn journal_decision(&self, operation: GuardianOperation) -> GuardianDecision {
        GuardianDecision::new(
            operation,
            self.records.clone(),
            self.verdict.clone(),
            self.database_marker.clone(),
        )
    }

    pub(crate) fn into_guardian_rejection(self) -> Option<GuardianRejection> {
        match self.verdict {
            GuardianVerdict::Accept => None,
            GuardianVerdict::Reject(rejection) => Some(
                AgentGuardianRejection::from_reject(rejection, self.records, self.database_marker)
                    .into_guardian_rejection(),
            ),
        }
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

    pub fn into_guardian_rejection(self) -> GuardianRejection {
        GuardianRejection {
            guardian_rejection_reason: self.reason,
            record_set: self.records,
            explanation: self.explanation,
            database_marker: self.database_marker,
        }
    }
}

struct GuardianRetry {
    completion: CompletionText,
    error: String,
}

struct GuardianOperationPrompt<'operation> {
    operation: &'operation GuardianOperation,
}

impl GuardianRetry {
    fn new(completion: CompletionText, error: String) -> Self {
        Self { completion, error }
    }

    fn user_text(&self) -> String {
        format!(
            "\n\nPrevious response was not a GuardianVerdict:\n{}\n\nParse error:\n{}\n\nReturn only the corrected GuardianVerdict NOTA value.",
            self.completion.payload(),
            self.error
        )
    }
}

impl<'operation> GuardianOperationPrompt<'operation> {
    fn new(operation: &'operation GuardianOperation) -> Self {
        Self { operation }
    }

    fn to_text(&self) -> String {
        match self.operation {
            GuardianOperation::Record(entry) => {
                format!("Record entry:\n{}", entry.to_nota())
            }
            GuardianOperation::Propose(entry) => {
                format!("Propose entry:\n{}", entry.to_nota())
            }
            GuardianOperation::Clarify(clarification) => {
                format!("Clarify request:\n{}", clarification.to_nota())
            }
            GuardianOperation::Supersede(supersession) => {
                format!("Supersede request:\n{}", supersession.to_nota())
            }
            GuardianOperation::Retire(retirement) => {
                format!("Retire request:\n{}", retirement.to_nota())
            }
        }
    }
}
