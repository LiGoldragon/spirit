use std::{
    io::Write,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use nota::NotaSource;
use signal_agent::{CompletionText, Input as AgentInput, Output as AgentOutput, Prompt};
use signal_spirit::SpiritGuardianAgentConfiguration;
use thiserror::Error;
use triad_runtime::{FrameBody, LengthPrefixedCodec};

use crate::{
    guardian_journal::{GuardianDecision, GuardianOperation},
    guardian_prompt::{GuardianPromptBuilder, GuardianPromptSource, GuardianRetry},
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
    prompt_source: GuardianPromptSource,
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
    pub const LOCAL_OPENAI_COMPATIBLE_PROVIDER: &'static str = "local-openai";
    pub const LOCAL_OPENAI_COMPATIBLE_MODEL: &'static str = "gpt-5.5";
    pub const LOCAL_OPENAI_COMPATIBLE_ENDPOINT: &'static str = "http://127.0.0.1:18080/v1";
    pub const DEFAULT_TIMEOUT_MILLISECONDS: u64 = 180_000;

    pub fn from_contract(configuration: &SpiritGuardianAgentConfiguration) -> Self {
        Self {
            socket_path: PathBuf::from(configuration.agent_socket_path()),
            provider_name: Some(
                configuration
                    .provider_name()
                    .unwrap_or(Self::LOCAL_OPENAI_COMPATIBLE_PROVIDER)
                    .to_owned(),
            ),
            model_name: Some(
                configuration
                    .model_name()
                    .unwrap_or(Self::LOCAL_OPENAI_COMPATIBLE_MODEL)
                    .to_owned(),
            ),
            timeout: Duration::from_millis(configuration.timeout_milliseconds()),
            maximum_output_tokens: configuration.maximum_output_tokens(),
            // The startup configuration archive carries no prompt field: a
            // freshly built guardian always starts on its compiled-in
            // (acknowledged strict-bar) role. An owner swaps the role live
            // through the meta `Configure` path, which calls `set_prompt_source`
            // on the installed guardian — no rebuild, no config-archive redeploy.
            prompt_source: GuardianPromptSource::compiled_in(),
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
            provider_name: Some(
                provider_name.unwrap_or_else(|| Self::LOCAL_OPENAI_COMPATIBLE_PROVIDER.to_owned()),
            ),
            model_name: Some(
                model_name.unwrap_or_else(|| Self::LOCAL_OPENAI_COMPATIBLE_MODEL.to_owned()),
            ),
            timeout,
            maximum_output_tokens,
            // No runtime override by default: callers that construct the
            // configuration directly get the compiled-in guardian prompt and
            // opt into a runtime directory through `with_prompt_source`.
            prompt_source: GuardianPromptSource::compiled_in(),
        }
    }

    pub fn local_openai_compatible(socket_path: impl Into<PathBuf>) -> Self {
        Self::new(
            socket_path,
            Some(Self::LOCAL_OPENAI_COMPATIBLE_PROVIDER.to_owned()),
            Some(Self::LOCAL_OPENAI_COMPATIBLE_MODEL.to_owned()),
            Duration::from_millis(Self::DEFAULT_TIMEOUT_MILLISECONDS),
            None,
        )
    }

    /// Install a runtime prompt source, overlaying the guardian's prose from a
    /// directory of section files while keeping the compiled-in default for any
    /// absent section.
    pub fn with_prompt_source(mut self, prompt_source: GuardianPromptSource) -> Self {
        self.prompt_source = prompt_source;
        self
    }
}

impl AgentGuardian {
    pub fn new(configuration: AgentGuardianConfiguration) -> Self {
        Self { configuration }
    }

    /// Swap the live guardian's prompt source. The engine calls this when an
    /// owner `Configure` carries a `GuardianPromptTarget`, so the role section
    /// the next verdict renders changes without a rebuild. The next
    /// `prompt_builder` reads the new source; no in-flight verdict is affected.
    pub fn set_prompt_source(&mut self, prompt_source: GuardianPromptSource) {
        self.configuration.prompt_source = prompt_source;
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
            match self.parse_verdict(&completion.completion_text) {
                Ok(verdict) => return Ok(verdict),
                Err(error) => {
                    retry = Some(GuardianRetry::new(
                        completion.completion_text,
                        error.to_string(),
                    ));
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
            match self.parse_referent_verdict(&completion.completion_text) {
                Ok(verdict) => return Ok(verdict),
                Err(error) => {
                    retry = Some(GuardianRetry::new(
                        completion.completion_text,
                        error.to_string(),
                    ));
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
            &self.configuration.prompt_source,
        )
    }

    /// The intent-guardian system prompt this guardian will currently send. A
    /// diagnostic affordance over the live prompt source, so the active role
    /// after a `set_prompt_source` swap is observable without a live model call.
    pub(crate) fn intent_guardian_system_prompt(&self) -> String {
        self.prompt_builder().intent_guardian_system_prompt()
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

    pub fn provider_name(&self) -> Option<&str> {
        self.provider_name.as_deref()
    }

    pub fn model_name(&self) -> Option<&str> {
        self.model_name.as_deref()
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

pub type AgentJudge = AgentGuardian;
pub type AgentJudgeConfiguration = AgentGuardianConfiguration;
pub type AgentJudgeDecision = AgentGuardianDecision;
pub type AgentJudgeError = AgentGuardianError;
pub type AgentJudgeRejection = AgentGuardianRejection;

#[cfg(test)]
mod tests {
    use super::*;
    use signal_spirit::{ConfigurationPath, SpiritGuardianTimeoutMilliseconds};

    #[test]
    fn contract_configuration_defaults_to_local_openai_compatible_judge() {
        let configuration = SpiritGuardianAgentConfiguration::new(
            ConfigurationPath::new("/tmp/agent.sock"),
            None,
            None,
            SpiritGuardianTimeoutMilliseconds::new(120_000),
            None,
        );

        let judge = AgentGuardianConfiguration::from_contract(&configuration);

        assert_eq!(
            judge.provider_name(),
            Some(AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_PROVIDER),
            "omitted provider resolves to the local OpenAI-compatible judge provider"
        );
        assert_eq!(
            judge.model_name(),
            Some(AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_MODEL),
            "omitted model resolves to gpt-5.5"
        );
    }

    #[test]
    fn direct_configuration_defaults_to_local_openai_compatible_judge() {
        let judge = AgentGuardianConfiguration::new(
            "/tmp/agent.sock",
            None,
            None,
            Duration::from_secs(120),
            None,
        );

        assert_eq!(
            judge.provider_name(),
            Some(AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_PROVIDER)
        );
        assert_eq!(
            judge.model_name(),
            Some(AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_MODEL)
        );
    }

    #[test]
    fn explicit_deepseek_configuration_stays_compatible() {
        let configuration = SpiritGuardianAgentConfiguration::new(
            ConfigurationPath::new("/tmp/agent.sock"),
            Some(signal_spirit::SpiritGuardianProviderName::new("deepseek")),
            Some(signal_spirit::SpiritGuardianModelName::new(
                "deepseek-v4-flash",
            )),
            SpiritGuardianTimeoutMilliseconds::new(120_000),
            None,
        );

        let judge = AgentGuardianConfiguration::from_contract(&configuration);

        assert_eq!(judge.provider_name(), Some("deepseek"));
        assert_eq!(judge.model_name(), Some("deepseek-v4-flash"));
    }
}
