use std::{
    io::Write,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use nota_next::NotaSource;
use signal_agent::{CompletionText, Input as AgentInput, Output as AgentOutput, Prompt};
use signal_spirit::SpiritGuardianAgentConfiguration;
use thiserror::Error;
use triad_runtime::{FrameBody, LengthPrefixedCodec};

use crate::{
    guardian_journal::{GuardianDecision, GuardianOperation},
    guardian_prompt::{GuardianPromptBuilder, GuardianRetry},
    schema::{
        nexus::{GuardianVerdict, ReferentGuardianVerdict, Reject, RejectReferent},
        signal::{
            DatabaseMarker, Explanation, GuardianRejection, GuardianRejectionReason, RecordSet,
            ReferentGuardianRejection, ReferentGuardianRejectionReason, ReferentRegistration,
            RegisteredReferents,
        },
    },
};

/// Format-correction retries after the initial guardian call. Even a strong
/// thinking model slips the double-nested `(Reject ( <Reason> [..] ))` verdict
/// shape occasionally; each retry feeds back the malformed text plus the parse
/// error, so a transient format slip is corrected rather than fail-closed into a
/// spurious reject. The agent daemon already does its own one-shot NOTA-parse
/// retry underneath this, so these are verdict-TYPE corrections on top.
const GUARDIAN_FORMAT_RETRIES: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentGuardianConfiguration {
    socket_path: PathBuf,
    provider_name: Option<String>,
    model_name: Option<String>,
    timeout: Duration,
    maximum_output_tokens: Option<u64>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentReferentGuardianDecision {
    verdict: ReferentGuardianVerdict,
    registered_referents: RegisteredReferents,
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
        maximum_output_tokens: Option<u64>,
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
        // Empty testimony is a structural fact, not a semantic judgement: a
        // candidate with no verbatim quote at all has produced no evidence, and
        // the flash-vs-pro eval showed even a strong model intermittently
        // overlooks an empty testimony vector. Reject it deterministically and
        // skip the model call (the guardian prompt still teaches MissingTestimony
        // for the semantic bare-affirmation-without-antecedent case).
        if operation.testimony_is_empty() {
            let verdict = GuardianVerdict::reject(Reject {
                guardian_rejection_reason: GuardianRejectionReason::MissingTestimony,
                explanation: Explanation::new("the justification carries no verbatim testimony"),
            });
            return AgentGuardianDecision::new(verdict, records, database_marker);
        }
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

    pub(crate) fn guard_referent(
        &self,
        registration: &ReferentRegistration,
        registered_referents: RegisteredReferents,
        database_marker: DatabaseMarker,
    ) -> AgentReferentGuardianDecision {
        let verdict = self
            .call_referent_guardian(registration, &registered_referents)
            .unwrap_or_else(|error| {
                ReferentGuardianVerdict::from_harness_rejection(
                    error.referent_guardian_rejection_reason(),
                    Explanation::new(error.to_string()),
                )
            });
        AgentReferentGuardianDecision::new(verdict, registered_referents, database_marker)
    }

    fn call_guardian(
        &self,
        operation: &GuardianOperation,
        records: &RecordSet,
    ) -> Result<GuardianVerdict, AgentGuardianError> {
        let prompts = self.prompt_builder();
        let mut retry: Option<GuardianRetry> = None;
        let mut last_error: Option<AgentGuardianError> = None;
        for _ in 0..=GUARDIAN_FORMAT_RETRIES {
            let output =
                self.call_agent(prompts.guardian_prompt(operation, records, retry.as_ref()))?;
            let AgentOutput::Completed(completion) = output else {
                return Err(AgentGuardianError::AgentRejected(format!("{output:?}")));
            };
            match self.parse_verdict(&completion.text) {
                Ok(verdict) => return Ok(verdict),
                Err(error) => {
                    retry = Some(GuardianRetry::new(completion.text, error.to_string()));
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.expect("at least one guardian attempt always runs"))
    }

    fn call_referent_guardian(
        &self,
        registration: &ReferentRegistration,
        registered_referents: &RegisteredReferents,
    ) -> Result<ReferentGuardianVerdict, AgentGuardianError> {
        let prompts = self.prompt_builder();
        let mut retry: Option<GuardianRetry> = None;
        let mut last_error: Option<AgentGuardianError> = None;
        for _ in 0..=GUARDIAN_FORMAT_RETRIES {
            let output = self.call_agent(prompts.referent_prompt(
                registration,
                registered_referents,
                retry.as_ref(),
            ))?;
            let AgentOutput::Completed(completion) = output else {
                return Err(AgentGuardianError::AgentRejected(format!("{output:?}")));
            };
            match self.parse_referent_verdict(&completion.text) {
                Ok(verdict) => return Ok(verdict),
                Err(error) => {
                    retry = Some(GuardianRetry::new(completion.text, error.to_string()));
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.expect("at least one referent guardian attempt always runs"))
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

    fn prompt_builder(&self) -> GuardianPromptBuilder<'_> {
        GuardianPromptBuilder::new(
            self.configuration.provider_name.as_deref(),
            self.configuration.model_name.as_deref(),
            self.configuration.maximum_output_tokens,
        )
    }

    fn parse_verdict(
        &self,
        completion: &CompletionText,
    ) -> Result<GuardianVerdict, AgentGuardianError> {
        NotaSource::new(completion.payload())
            .parse::<GuardianVerdict>()
            .map_err(|error| AgentGuardianError::Malformed(error.to_string()))
    }

    fn parse_referent_verdict(
        &self,
        completion: &CompletionText,
    ) -> Result<ReferentGuardianVerdict, AgentGuardianError> {
        NotaSource::new(completion.payload())
            .parse::<ReferentGuardianVerdict>()
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

    fn referent_guardian_rejection_reason(&self) -> ReferentGuardianRejectionReason {
        match self {
            Self::Socket(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                ReferentGuardianRejectionReason::HarnessTimedOut
            }
            Self::Socket(_) | Self::Frame(_) => ReferentGuardianRejectionReason::HarnessUnavailable,
            Self::AgentRejected(_) | Self::Malformed(_) => {
                ReferentGuardianRejectionReason::HarnessMalformed
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
        GuardianDecision::record(
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

impl AgentReferentGuardianDecision {
    fn new(
        verdict: ReferentGuardianVerdict,
        registered_referents: RegisteredReferents,
        database_marker: DatabaseMarker,
    ) -> Self {
        Self {
            verdict,
            registered_referents,
            database_marker,
        }
    }

    pub(crate) fn journal_decision(&self, registration: ReferentRegistration) -> GuardianDecision {
        GuardianDecision::referent(
            registration,
            self.registered_referents.clone(),
            self.verdict.clone(),
            self.database_marker.clone(),
        )
    }

    pub(crate) fn into_guardian_rejection(self) -> Option<ReferentGuardianRejection> {
        match self.verdict {
            ReferentGuardianVerdict::Accept => None,
            ReferentGuardianVerdict::RejectReferent(rejection) => Some(
                AgentReferentGuardianRejection::from_reject(
                    rejection,
                    self.registered_referents,
                    self.database_marker,
                )
                .into_guardian_rejection(),
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AgentReferentGuardianRejection {
    reason: ReferentGuardianRejectionReason,
    registered_referents: RegisteredReferents,
    explanation: Explanation,
    database_marker: DatabaseMarker,
}

impl AgentReferentGuardianRejection {
    fn from_reject(
        rejection: RejectReferent,
        registered_referents: RegisteredReferents,
        database_marker: DatabaseMarker,
    ) -> Self {
        Self {
            reason: rejection.referent_guardian_rejection_reason,
            registered_referents,
            explanation: rejection.explanation,
            database_marker,
        }
    }

    fn into_guardian_rejection(self) -> ReferentGuardianRejection {
        ReferentGuardianRejection {
            referent_guardian_rejection_reason: self.reason,
            registered_referents: self.registered_referents,
            explanation: self.explanation,
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
        }
    }
}
