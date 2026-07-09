use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use signal_frame::{
    ExchangeFrameBody, ExchangeIdentifier, ExchangeLane, LaneSequence, Reply, Request,
    SessionEpoch, ShortHeader, SubReply,
};
use signal_spirit::SpiritGuardianAgentConfiguration;
use signal_spirit_judge::{
    AdmissionJudgeOperation, AdmissionJudgePacket, AdmissionJudgeResponse, AdmissionJudgeVerdict,
    AdmissionRejectionReason, JudgeDiagnostic, JudgmentScope, ReferentRegistrationJudgePacket,
    ReferentRegistrationJudgeResponse, ReferentRegistrationJudgeVerdict,
    ReferentRegistrationRejectionReason, SpiritJudgeFrame, SpiritJudgeReply, SpiritJudgeRequest,
    SpiritJudgeRequestRejection, SpiritJudgeRequestRejectionReason,
};
use thiserror::Error;

use crate::{
    guardian_journal::{GuardianDecision, GuardianOperation},
    schema::{
        nexus::{GuardianVerdict, ReferentGuardianVerdict, Reject, RejectReferent},
        signal::{
            DatabaseMarker, Entry, Explanation, GuardianRejection, GuardianRejectionReason,
            Magnitude, RecordSet, ReferentGuardianRejection, ReferentGuardianRejectionReason,
            ReferentRegistration, RegisteredReferents,
        },
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentGuardianConfiguration {
    socket_path: PathBuf,
    timeout: Duration,
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
    #[error("spirit judge socket unavailable: {0}")]
    Socket(std::io::Error),

    #[error("spirit judge frame failed: {0}")]
    Frame(String),

    #[error("spirit judge replied with the wrong operation: {0}")]
    WrongReply(&'static str),
}

impl AgentGuardianConfiguration {
    pub const LOCAL_OPENAI_COMPATIBLE_PROVIDER: &'static str = "local-openai";
    pub const LOCAL_OPENAI_COMPATIBLE_MODEL: &'static str = "gpt-5.4-mini";
    pub const LOCAL_OPENAI_COMPATIBLE_ENDPOINT: &'static str = "http://127.0.0.1:18080/v1";
    pub const DEFAULT_TIMEOUT_MILLISECONDS: u64 = 180_000;

    pub fn from_contract(configuration: &SpiritGuardianAgentConfiguration) -> Self {
        Self {
            socket_path: PathBuf::from(configuration.agent_socket_path()),
            timeout: Duration::from_millis(configuration.timeout_milliseconds()),
        }
    }

    pub fn new(
        socket_path: impl Into<PathBuf>,
        _provider_name: Option<String>,
        _model_name: Option<String>,
        timeout: Duration,
        _maximum_output_tokens: Option<u64>,
    ) -> Self {
        Self {
            socket_path: socket_path.into(),
            timeout,
        }
    }

    pub fn local_openai_compatible(socket_path: impl Into<PathBuf>) -> Self {
        Self::new(
            socket_path,
            None,
            None,
            Duration::from_millis(Self::DEFAULT_TIMEOUT_MILLISECONDS),
            None,
        )
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn provider_name(&self) -> Option<&str> {
        None
    }

    pub fn model_name(&self) -> Option<&str> {
        None
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
        let packet = AdmissionJudgePacket::new(
            JudgmentScopeProjection::new(operation).into_contract(),
            AdmissionJudgeOperationProjection::new(operation).into_contract(),
            records.clone(),
            database_marker.clone(),
        );
        let verdict = match self.call_judge(SpiritJudgeRequest::JudgeAdmission(packet)) {
            Ok(SpiritJudgeReply::AdmissionJudged(response)) => {
                GuardianVerdict::from_admission_response(response)
            }
            Ok(SpiritJudgeReply::RequestRejected(rejection)) => {
                GuardianVerdict::from_request_rejection(rejection)
            }
            Ok(SpiritJudgeReply::ReferentRegistrationJudged(_)) => {
                GuardianVerdict::reject(Reject {
                    guardian_rejection_reason: GuardianRejectionReason::HarnessMalformed,
                    explanation: Explanation::new(
                        "spirit judge returned a referent reply for an admission request",
                    ),
                })
            }
            Err(error) => GuardianVerdict::from_judge_error(error),
        };
        AgentGuardianDecision::new(verdict, records, database_marker)
    }

    pub(crate) fn guard_referent(
        &self,
        registration: &ReferentRegistration,
        registered_referents: RegisteredReferents,
        database_marker: DatabaseMarker,
    ) -> AgentReferentGuardianDecision {
        let packet = ReferentRegistrationJudgePacket::new(
            JudgmentScope::private_hashes_and_redaction(),
            registration.clone(),
            registered_referents.clone(),
            database_marker.clone(),
        );
        let verdict = match self.call_judge(SpiritJudgeRequest::JudgeReferentRegistration(packet)) {
            Ok(SpiritJudgeReply::ReferentRegistrationJudged(response)) => {
                ReferentGuardianVerdict::from_referent_response(response)
            }
            Ok(SpiritJudgeReply::RequestRejected(rejection)) => {
                ReferentGuardianVerdict::from_request_rejection(rejection)
            }
            Ok(SpiritJudgeReply::AdmissionJudged(_)) => {
                ReferentGuardianVerdict::reject_referent(RejectReferent {
                    referent_guardian_rejection_reason:
                        ReferentGuardianRejectionReason::HarnessMalformed,
                    explanation: Explanation::new(
                        "spirit judge returned an admission reply for a referent request",
                    ),
                })
            }
            Err(error) => ReferentGuardianVerdict::from_judge_error(error),
        };
        AgentReferentGuardianDecision::new(verdict, registered_referents, database_marker)
    }

    fn call_judge(
        &self,
        request: SpiritJudgeRequest,
    ) -> Result<SpiritJudgeReply, AgentGuardianError> {
        let mut stream = UnixStream::connect(self.configuration.socket_path())
            .map_err(AgentGuardianError::Socket)?;
        stream
            .set_read_timeout(Some(self.configuration.timeout()))
            .map_err(AgentGuardianError::Socket)?;
        stream
            .set_write_timeout(Some(self.configuration.timeout()))
            .map_err(AgentGuardianError::Socket)?;
        let frame = SpiritJudgeFrame::with_short_header(
            ShortHeader::empty(),
            ExchangeFrameBody::Request {
                exchange: ClientExchange::first().identifier(),
                request: Request::from_payload(request),
            },
        );
        let bytes = frame
            .encode_length_prefixed()
            .map_err(|error| AgentGuardianError::Frame(error.to_string()))?;
        stream
            .write_all(bytes.as_slice())
            .map_err(AgentGuardianError::Socket)?;
        stream.flush().map_err(AgentGuardianError::Socket)?;
        let reply_frame = FrameReader::new(&mut stream).read_reply_frame()?;
        ClientExchange::reply_from_frame(reply_frame)
    }
}

struct FrameReader<'stream> {
    stream: &'stream mut UnixStream,
}

impl<'stream> FrameReader<'stream> {
    fn new(stream: &'stream mut UnixStream) -> Self {
        Self { stream }
    }

    fn read_reply_frame(&mut self) -> Result<SpiritJudgeFrame, AgentGuardianError> {
        let mut prefix = [0_u8; 4];
        self.stream
            .read_exact(&mut prefix)
            .map_err(AgentGuardianError::Socket)?;
        let length = u32::from_be_bytes(prefix) as usize;
        let mut bytes = Vec::with_capacity(4 + length);
        bytes.extend_from_slice(&prefix);
        bytes.resize(4 + length, 0);
        self.stream
            .read_exact(&mut bytes[4..])
            .map_err(AgentGuardianError::Socket)?;
        SpiritJudgeFrame::decode_length_prefixed(bytes.as_slice())
            .map_err(|error| AgentGuardianError::Frame(error.to_string()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClientExchange {
    session_epoch: SessionEpoch,
    sequence: LaneSequence,
}

impl ClientExchange {
    fn first() -> Self {
        Self {
            session_epoch: SessionEpoch::new(1),
            sequence: LaneSequence::first(),
        }
    }

    fn identifier(&self) -> ExchangeIdentifier {
        ExchangeIdentifier::new(self.session_epoch, ExchangeLane::Connector, self.sequence)
    }

    fn reply_from_frame(frame: SpiritJudgeFrame) -> Result<SpiritJudgeReply, AgentGuardianError> {
        match frame.into_body() {
            ExchangeFrameBody::Reply { reply, .. } => match reply {
                Reply::Accepted { per_operation, .. } => {
                    Ok(per_operation.into_head().representative_reply())
                }
                Reply::Rejected { reason } => Err(AgentGuardianError::Frame(reason.to_string())),
            },
            ExchangeFrameBody::HandshakeRequest(_) => {
                Err(AgentGuardianError::WrongReply("handshake request"))
            }
            ExchangeFrameBody::HandshakeReply(_) => {
                Err(AgentGuardianError::WrongReply("handshake reply"))
            }
            ExchangeFrameBody::Request { .. } => Err(AgentGuardianError::WrongReply("request")),
        }
    }
}

trait RepresentativeReply {
    fn representative_reply(self) -> SpiritJudgeReply;
}

impl RepresentativeReply for SubReply<SpiritJudgeReply> {
    fn representative_reply(self) -> SpiritJudgeReply {
        match self {
            Self::Ok(reply) => reply,
            Self::Failed { detail, .. } => detail.unwrap_or_else(|| {
                SpiritJudgeReply::RequestRejected(SpiritJudgeRequestRejection::new(
                    SpiritJudgeRequestRejectionReason::InvalidRequest,
                    JudgeDiagnostic::redacted(
                        signal_spirit_judge::RedactedText::new(
                            "spirit judge frame operation failed",
                        )
                        .expect("static diagnostic is non-empty"),
                    ),
                ))
            }),
            Self::Invalidated | Self::Skipped => {
                SpiritJudgeReply::RequestRejected(SpiritJudgeRequestRejection::new(
                    SpiritJudgeRequestRejectionReason::InvalidRequest,
                    JudgeDiagnostic::redacted(
                        signal_spirit_judge::RedactedText::new(
                            "spirit judge frame operation produced no reply",
                        )
                        .expect("static diagnostic is non-empty"),
                    ),
                ))
            }
        }
    }
}

struct AdmissionJudgeOperationProjection<'operation> {
    operation: &'operation GuardianOperation,
}

impl<'operation> AdmissionJudgeOperationProjection<'operation> {
    fn new(operation: &'operation GuardianOperation) -> Self {
        Self { operation }
    }

    fn into_contract(self) -> AdmissionJudgeOperation {
        match self.operation {
            GuardianOperation::Record(request) => AdmissionJudgeOperation::Record(request.clone()),
            GuardianOperation::Propose(proposal) => {
                AdmissionJudgeOperation::Propose(proposal.clone())
            }
            GuardianOperation::Clarify(clarification) => {
                AdmissionJudgeOperation::Clarify(clarification.clone())
            }
            GuardianOperation::ResolveClarification(resolution) => {
                AdmissionJudgeOperation::ResolveClarification(resolution.clone())
            }
            GuardianOperation::Supersede(supersession) => {
                AdmissionJudgeOperation::Supersede(supersession.clone())
            }
            GuardianOperation::Retire(retirement) => {
                AdmissionJudgeOperation::Retire(retirement.clone())
            }
            GuardianOperation::ChangeRecord(change) => {
                AdmissionJudgeOperation::ChangeRecord(change.clone())
            }
        }
    }
}

struct JudgmentScopeProjection<'operation> {
    operation: &'operation GuardianOperation,
}

impl<'operation> JudgmentScopeProjection<'operation> {
    fn new(operation: &'operation GuardianOperation) -> Self {
        Self { operation }
    }

    fn into_contract(self) -> JudgmentScope {
        if self
            .operation
            .candidate_entries()
            .iter()
            .any(|entry| entry.is_private())
        {
            JudgmentScope::private_hashes_and_redaction()
        } else {
            JudgmentScope::public()
        }
    }
}

trait EntryPrivacy {
    fn is_private(&self) -> bool;
}

impl EntryPrivacy for &Entry {
    fn is_private(&self) -> bool {
        self.privacy.payload() != &Magnitude::Zero
    }
}

impl GuardianVerdict {
    fn from_admission_response(response: AdmissionJudgeResponse) -> Self {
        match response.verdict {
            AdmissionJudgeVerdict::Accept => Self::Accept,
            AdmissionJudgeVerdict::Reject(reason) => Self::reject(Reject {
                guardian_rejection_reason: AdmissionRejectionProjection::new(reason).into_signal(),
                explanation: JudgeDiagnosticProjection::new(response.diagnostic).into_explanation(),
            }),
        }
    }

    fn from_request_rejection(rejection: SpiritJudgeRequestRejection) -> Self {
        Self::reject(Reject {
            guardian_rejection_reason: RequestRejectionProjection::new(rejection.reason)
                .to_guardian_reason(),
            explanation: JudgeDiagnosticProjection::new(rejection.diagnostic).into_explanation(),
        })
    }

    fn from_judge_error(error: AgentGuardianError) -> Self {
        Self::reject(Reject {
            guardian_rejection_reason: error.guardian_rejection_reason(),
            explanation: Explanation::new(error.to_string()),
        })
    }
}

impl ReferentGuardianVerdict {
    fn from_referent_response(response: ReferentRegistrationJudgeResponse) -> Self {
        match response.verdict {
            ReferentRegistrationJudgeVerdict::Accept => Self::Accept,
            ReferentRegistrationJudgeVerdict::RejectReferent(reason) => {
                Self::reject_referent(RejectReferent {
                    referent_guardian_rejection_reason: ReferentRejectionProjection::new(reason)
                        .into_signal(),
                    explanation: JudgeDiagnosticProjection::new(response.diagnostic)
                        .into_explanation(),
                })
            }
        }
    }

    fn from_request_rejection(rejection: SpiritJudgeRequestRejection) -> Self {
        Self::reject_referent(RejectReferent {
            referent_guardian_rejection_reason: RequestRejectionProjection::new(rejection.reason)
                .to_referent_reason(),
            explanation: JudgeDiagnosticProjection::new(rejection.diagnostic).into_explanation(),
        })
    }

    fn from_judge_error(error: AgentGuardianError) -> Self {
        Self::reject_referent(RejectReferent {
            referent_guardian_rejection_reason: error.referent_guardian_rejection_reason(),
            explanation: Explanation::new(error.to_string()),
        })
    }
}

struct JudgeDiagnosticProjection {
    diagnostic: JudgeDiagnostic,
}

impl JudgeDiagnosticProjection {
    fn new(diagnostic: JudgeDiagnostic) -> Self {
        Self { diagnostic }
    }

    fn into_explanation(self) -> Explanation {
        if self.diagnostic.content_hashes.is_empty() {
            Explanation::new(self.diagnostic.redacted_text.as_str())
        } else {
            let hashes = self
                .diagnostic
                .content_hashes
                .iter()
                .map(|hash| hash.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Explanation::new(format!(
                "{} [{}]",
                self.diagnostic.redacted_text.as_str(),
                hashes
            ))
        }
    }
}

struct AdmissionRejectionProjection {
    reason: AdmissionRejectionReason,
}

impl AdmissionRejectionProjection {
    fn new(reason: AdmissionRejectionReason) -> Self {
        Self { reason }
    }

    fn into_signal(self) -> GuardianRejectionReason {
        match self.reason {
            AdmissionRejectionReason::Duplicate => GuardianRejectionReason::Duplicate,
            AdmissionRejectionReason::Contradiction => GuardianRejectionReason::Contradiction,
            AdmissionRejectionReason::Compound => GuardianRejectionReason::Compound,
            AdmissionRejectionReason::NonIntent => GuardianRejectionReason::NonIntent,
            AdmissionRejectionReason::NegativeGuideline => {
                GuardianRejectionReason::NegativeGuideline
            }
            AdmissionRejectionReason::Matter => GuardianRejectionReason::Matter,
            AdmissionRejectionReason::UnclearPrivacy => GuardianRejectionReason::UnclearPrivacy,
            AdmissionRejectionReason::UnclearDomain => GuardianRejectionReason::UnclearDomain,
            AdmissionRejectionReason::ClarifyTramples => GuardianRejectionReason::ClarifyTramples,
            AdmissionRejectionReason::ClarifyLosesMeaning => {
                GuardianRejectionReason::ClarifyLosesMeaning
            }
            AdmissionRejectionReason::SupersedeTargetMissing => {
                GuardianRejectionReason::SupersedeTargetMissing
            }
            AdmissionRejectionReason::RetrievalInsufficient => {
                GuardianRejectionReason::RetrievalInsufficient
            }
            AdmissionRejectionReason::MissingTestimony => GuardianRejectionReason::MissingTestimony,
            AdmissionRejectionReason::TestimonyFabricated => {
                GuardianRejectionReason::TestimonyFabricated
            }
            AdmissionRejectionReason::InsufficientWarrant => {
                GuardianRejectionReason::InsufficientWarrant
            }
            AdmissionRejectionReason::Overstated => GuardianRejectionReason::Overstated,
            AdmissionRejectionReason::ImportanceUnsupported => {
                GuardianRejectionReason::ImportanceUnsupported
            }
            AdmissionRejectionReason::JudgeUnavailable => {
                GuardianRejectionReason::HarnessUnavailable
            }
            AdmissionRejectionReason::JudgeMalformed => GuardianRejectionReason::HarnessMalformed,
            AdmissionRejectionReason::JudgeTimedOut => GuardianRejectionReason::HarnessTimedOut,
        }
    }
}

struct RequestRejectionProjection {
    reason: SpiritJudgeRequestRejectionReason,
}

impl RequestRejectionProjection {
    fn new(reason: SpiritJudgeRequestRejectionReason) -> Self {
        Self { reason }
    }

    fn to_guardian_reason(&self) -> GuardianRejectionReason {
        match self.reason {
            SpiritJudgeRequestRejectionReason::InvalidRequest
            | SpiritJudgeRequestRejectionReason::ConfigurationUnavailable
            | SpiritJudgeRequestRejectionReason::ResponseFormatFailure => {
                GuardianRejectionReason::HarnessMalformed
            }
            SpiritJudgeRequestRejectionReason::ProviderUnavailable
            | SpiritJudgeRequestRejectionReason::ProviderRejected => {
                GuardianRejectionReason::HarnessUnavailable
            }
        }
    }

    fn to_referent_reason(&self) -> ReferentGuardianRejectionReason {
        match self.reason {
            SpiritJudgeRequestRejectionReason::InvalidRequest
            | SpiritJudgeRequestRejectionReason::ConfigurationUnavailable
            | SpiritJudgeRequestRejectionReason::ResponseFormatFailure => {
                ReferentGuardianRejectionReason::HarnessMalformed
            }
            SpiritJudgeRequestRejectionReason::ProviderUnavailable
            | SpiritJudgeRequestRejectionReason::ProviderRejected => {
                ReferentGuardianRejectionReason::HarnessUnavailable
            }
        }
    }
}

struct ReferentRejectionProjection {
    reason: ReferentRegistrationRejectionReason,
}

impl ReferentRejectionProjection {
    fn new(reason: ReferentRegistrationRejectionReason) -> Self {
        Self { reason }
    }

    fn into_signal(self) -> ReferentGuardianRejectionReason {
        match self.reason {
            ReferentRegistrationRejectionReason::Duplicate => {
                ReferentGuardianRejectionReason::Duplicate
            }
            ReferentRegistrationRejectionReason::Ambiguous => {
                ReferentGuardianRejectionReason::Ambiguous
            }
            ReferentRegistrationRejectionReason::TooVague => {
                ReferentGuardianRejectionReason::TooVague
            }
            ReferentRegistrationRejectionReason::AliasCollision => {
                ReferentGuardianRejectionReason::AliasCollision
            }
            ReferentRegistrationRejectionReason::NonReferent => {
                ReferentGuardianRejectionReason::NonReferent
            }
            ReferentRegistrationRejectionReason::UnclearJustification => {
                ReferentGuardianRejectionReason::UnclearJustification
            }
            ReferentRegistrationRejectionReason::JudgeUnavailable => {
                ReferentGuardianRejectionReason::HarnessUnavailable
            }
            ReferentRegistrationRejectionReason::JudgeMalformed => {
                ReferentGuardianRejectionReason::HarnessMalformed
            }
            ReferentRegistrationRejectionReason::JudgeTimedOut => {
                ReferentGuardianRejectionReason::HarnessTimedOut
            }
        }
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
            Self::Socket(_) | Self::Frame(_) | Self::WrongReply(_) => {
                GuardianRejectionReason::HarnessUnavailable
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
            Self::Socket(_) | Self::Frame(_) | Self::WrongReply(_) => {
                ReferentGuardianRejectionReason::HarnessUnavailable
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
