//! Spirit mirror ORIGINATION — hand the authorized head to the LOCAL router.
//!
//! The dual of [`crate::apply_ingress`]. After the 1-of-1 LOCAL criome gate
//! authorizes the post-commit head, Spirit projects
//! `{record_identifier, versioned entry, Evidence}` into a `signal-spirit`
//! `ApplyAuthorizedRecord` frame, wraps that frame as ONE
//! [`RoutedContractObject`], and hands it to its LOCAL router's working socket
//! as a `signal-router` `SubmitRoutedObjects` origination. The router carries
//! the object to the peer node and delivers the raw frame octets to the peer
//! Spirit's working socket, where [`crate::apply_ingress`] re-judges the carried
//! Evidence and lands the record live.
//!
//! Two invariants make the far-side apply succeed by construction:
//!
//!   - The `versioned_entry_hex` is the byte-for-byte
//!     [`crate::store::Store::versioned_log_head_object`] octets, so it re-hashes
//!     to the same head digest on the peer.
//!   - The `authorized_evidence_hex` is the SAME
//!     [`signal_criome::Evidence`] the local criome authorized, whose
//!     `operation` is the head's operation digest — so the peer's content-address
//!     binding (`evidence.operation == entry_digest`) holds.
//!
//! The router hand-off dials the router's working socket synchronously (the
//! caller wraps it in `spawn_blocking`), mirroring the criome gate's
//! [`criome::transport::CriomeClient::send`]. The origination is best-effort
//! relative to LOCAL durability: the working write already committed before the
//! gate runs, so an unreachable or refusing router leaves the local commit
//! intact and the head waits for the next authorized drain (a durable outbox and
//! redial are a later milestone).

use std::{
    io::Write,
    os::unix::net::UnixStream,
    path::PathBuf,
    time::{Duration, Instant},
};

use criome::transport::CriomeClient;
use rkyv::rancor;
use sema_engine::VersionedCommitLogEntry;
use signal_criome::{
    AuthorizedObjectReference, ContractDigest, CriomeReply, CriomeRequest, Evidence,
    QuorumProposal, QuorumRoundIdentifier, QuorumRoundState, QuorumRoundStatus, TimeWindow,
    TimestampNanos,
};
use signal_frame::{ExchangeIdentifier, ExchangeLane, LaneSequence, Reply, SessionEpoch, SubReply};
use signal_router::{
    ActorIdentifier, ContractName, ContractOperation, ContractPayloadSize, ForwardedMessagePayload,
    Frame as RouterFrame, FrameBody as RouterFrameBody, Input as RouterInput,
    Integer as RouterInteger, Output as RouterOutput, RoutedContractObject,
};
use thiserror::Error;
use triad_runtime::{
    FrameBody as LengthPrefixedFrameBody, FrameError as LengthPrefixedFrameError,
    LengthPrefixedCodec,
};

use crate::schema::{
    sema::StoredRecord,
    signal::{
        AuthorizedEvidenceHex, AuthorizedRecordApplication, Input as SpiritInput, RecordIdentifier,
        VersionedEntryHex,
    },
};

/// The `contract_name` label stamped on the routed object. The router relays the
/// octets payload-blind; the name is an attestation/audit label naming the
/// contract the octets belong to.
const SPIRIT_CONTRACT_NAME: &str = "signal-spirit";
/// The `contract_operation` label stamped on the routed object — the operation
/// the carried frame decodes to on the peer's working socket.
const APPLY_AUTHORIZED_RECORD_OPERATION: &str = "ApplyAuthorizedRecord";

/// The deploy-config origination target: the LOCAL router's working socket and
/// the source/destination actor identities the router routes the hand-off by.
///
/// Armed alongside the 1-of-1 criome gate. Unarmed means Spirit authorizes its
/// head but does not originate a forward — the head simply lands locally, with
/// no peer. This is a data-bearing config object: the socket and actor names
/// travel with it, so its projection and hand-off methods live here.
#[derive(Clone, Debug)]
pub struct RouterOrigination {
    router_socket: PathBuf,
    source_actor: ActorIdentifier,
    destination_actor: ActorIdentifier,
}

impl RouterOrigination {
    /// Configure origination against the LOCAL router working socket. The
    /// `source_actor` names this Spirit's router identity; the
    /// `destination_actor` names the peer Spirit the router resolves a remote
    /// route to.
    pub fn new(
        router_socket: impl Into<PathBuf>,
        source_actor: ActorIdentifier,
        destination_actor: ActorIdentifier,
    ) -> Self {
        Self {
            router_socket: router_socket.into(),
            source_actor,
            destination_actor,
        }
    }

    /// The configured LOCAL router working socket.
    pub fn router_socket(&self) -> &std::path::Path {
        &self.router_socket
    }

    /// Project the authorized head into the router hand-off payload: an
    /// `ApplyAuthorizedRecord` frame carried as ONE [`RoutedContractObject`]
    /// destined for the peer Spirit.
    ///
    /// `versioned_entry_octets` are the rkyv [`VersionedCommitLogEntry`] head
    /// object (`Store::versioned_log_head_object`); `evidence` is the SAME
    /// [`Evidence`] the local criome authorized. The record identifier is decoded
    /// from the carried entry so it matches what the apply ingress recovers, by
    /// construction — the origination and the apply read the identity from the
    /// same content-addressed bytes.
    pub fn submission_for_head(
        &self,
        versioned_entry_octets: Vec<u8>,
        evidence: Evidence,
    ) -> Result<ForwardedMessagePayload, RouterOriginationError> {
        let record_identifier = Self::head_record_identifier(&versioned_entry_octets)?;
        let evidence_octets = rkyv::to_bytes::<rancor::Error>(&evidence)
            .map_err(|_| RouterOriginationError::EvidenceEncode)?
            .to_vec();
        let application = AuthorizedRecordApplication {
            record_identifier: RecordIdentifier::new(record_identifier),
            versioned_entry_hex: VersionedEntryHex::new(Self::to_hex(&versioned_entry_octets)),
            authorized_evidence_hex: AuthorizedEvidenceHex::new(Self::to_hex(&evidence_octets)),
        };
        let frame_octets = SpiritInput::apply_authorized_record(application)
            .encode_signal_frame()
            .map_err(|source| RouterOriginationError::Frame {
                message: source.to_string(),
            })?;
        let routed_object = RoutedContractObject::new(
            ContractName::new(SPIRIT_CONTRACT_NAME),
            ContractOperation::new(APPLY_AUTHORIZED_RECORD_OPERATION),
            ContractPayloadSize::new(frame_octets.len() as RouterInteger),
            frame_octets.into_iter().map(RouterInteger::from).collect(),
        );
        Ok(ForwardedMessagePayload::new(
            self.source_actor.clone(),
            self.destination_actor.clone(),
            APPLY_AUTHORIZED_RECORD_OPERATION.to_owned(),
            Vec::new(),
            vec![routed_object],
        ))
    }

    /// Hand the payload to the LOCAL router's working socket as a
    /// `SubmitRoutedObjects` origination and confirm the router accepted it.
    ///
    /// Synchronous `UnixStream` round-trip (the caller wraps it in
    /// `spawn_blocking`): write the exchange-framed request, read the router's
    /// reply, and require `RoutedObjectsAccepted`. Any transport error, a router
    /// rejection, or an off-contract reply is a typed [`RouterOriginationError`].
    pub fn hand_off(&self, payload: ForwardedMessagePayload) -> Result<(), RouterOriginationError> {
        let mut stream = UnixStream::connect(&self.router_socket)?;
        let exchange = ExchangeIdentifier::new(
            SessionEpoch::new(0),
            ExchangeLane::Connector,
            LaneSequence::first(),
        );
        let request_octets = RouterInput::submit_routed_objects(payload)
            .into_frame(exchange)
            .encode()
            .map_err(|source| RouterOriginationError::Frame {
                message: source.to_string(),
            })?;
        let codec = LengthPrefixedCodec::default();
        codec.write_body(&mut stream, &LengthPrefixedFrameBody::new(request_octets))?;
        stream.flush()?;
        let reply_body = codec.read_body(&mut stream)?;
        let reply_frame = RouterFrame::decode(reply_body.bytes()).map_err(|source| {
            RouterOriginationError::Frame {
                message: source.to_string(),
            }
        })?;
        Self::accepted(reply_frame)
    }

    /// Confirm the router reply is a `RoutedObjectsAccepted` acceptance.
    fn accepted(frame: RouterFrame) -> Result<(), RouterOriginationError> {
        match frame.into_body() {
            RouterFrameBody::Reply { reply, .. } => match reply {
                Reply::Accepted { per_operation, .. } => match per_operation.into_head() {
                    SubReply::Ok(RouterOutput::RoutedObjectsAccepted(_)) => Ok(()),
                    other => Err(RouterOriginationError::UnexpectedReply {
                        got: format!("{other:?}"),
                    }),
                },
                Reply::Rejected { reason } => Err(RouterOriginationError::Rejected {
                    reason: reason.to_string(),
                }),
            },
            other => Err(RouterOriginationError::UnexpectedReply {
                got: format!("{other:?}"),
            }),
        }
    }

    /// Decode the record identifier the carried head commits — the head
    /// operation's rkyv [`StoredRecord`] identity. Mirrors the apply ingress's
    /// own decode so origination and apply agree on the identity by construction.
    fn head_record_identifier(
        versioned_entry_octets: &[u8],
    ) -> Result<String, RouterOriginationError> {
        let versioned_entry =
            rkyv::from_bytes::<VersionedCommitLogEntry, rancor::Error>(versioned_entry_octets)
                .map_err(|_| RouterOriginationError::MalformedHead)?;
        let payload_octets = versioned_entry
            .operations()
            .head()
            .payload()
            .bytes()
            .ok_or(RouterOriginationError::MalformedHead)?;
        let stored = rkyv::from_bytes::<StoredRecord, rancor::Error>(payload_octets)
            .map_err(|_| RouterOriginationError::MalformedHead)?;
        Ok(stored.record_identifier.into_payload())
    }

    /// Lowercase hex-encode octets for the `VersionedEntryHex` /
    /// `AuthorizedEvidenceHex` carriage — the inverse of the apply ingress's
    /// `CarriedHex::octets` decode.
    fn to_hex(octets: &[u8]) -> String {
        use std::fmt::Write as _;
        let mut text = String::with_capacity(octets.len() * 2);
        for byte in octets {
            let _ = write!(text, "{byte:02x}");
        }
        text
    }
}

/// The origination hand-off's typed failure modes. An unreachable or refusing
/// router is a real hand-off failure the daemon logs and drops (the local commit
/// already stands); a malformed head or an evidence-encode failure is a genuine
/// projection fault.
#[derive(Debug, Error)]
pub enum RouterOriginationError {
    #[error("router origination socket connect/io failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("router origination frame transport failed: {0}")]
    Transport(#[from] LengthPrefixedFrameError),

    #[error("router origination frame codec failed: {message}")]
    Frame { message: String },

    #[error("evidence archive encode failed")]
    EvidenceEncode,

    #[error("the versioned head entry is malformed")]
    MalformedHead,

    #[error("router refused the origination: {reason}")]
    Rejected { reason: String },

    #[error("router answered with an unexpected reply: {got}")]
    UnexpectedReply { got: String },
}

/// The bounded budget for awaiting a proposed quorum round's completion.
///
/// Criome does not yet push an authorized-object completion event to a socket
/// subscriber — its subscription registry accumulates updates but ships no
/// stream, so a consumer cannot subscribe for the verdict (building that push
/// primitive is a criome follow-up). Until it lands, the ship task awaits the
/// round on this budget: a completion await for a verdict that arrives promptly
/// when the peer is reachable, NOT a steady-state poll. A round that does not
/// reach a majority within the budget stays withheld — the change simply does not
/// ship, exactly the "unreachable peer ⇒ waits" behavior.
#[derive(Clone, Debug)]
pub struct QuorumCompletionBudget {
    deadline: Duration,
    interval: Duration,
}

impl QuorumCompletionBudget {
    pub fn new(deadline: Duration, interval: Duration) -> Self {
        Self { deadline, interval }
    }
}

impl Default for QuorumCompletionBudget {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(30),
            interval: Duration::from_millis(100),
        }
    }
}

/// The engine-visible result of running the quorum origination boundary for one
/// head. The daemon logs it; the meaningful states are `Shipped` (the round was
/// already a majority and the authorized head went to the router) and `Proposed`
/// (the round opened and a detached task awaits its completion off the mailbox).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuorumOriginationOutcome {
    /// The round reached a true majority on propose and the authorized head +
    /// assembled Evidence were handed to the LOCAL router.
    Shipped,
    /// The round opened (`Gathering`); a detached ship awaits its completion and
    /// hands the head to the router only on the quorum's `Authorized` verdict.
    Proposed,
    /// The LOCAL criome socket could not be reached — nothing was proposed, the
    /// local commit stands and the head waits for the next drain.
    Unreachable,
    /// No LOCAL criome socket, no admitted mirror contract, or origination is
    /// unarmed — there is no quorum boundary to run. The head lands locally with
    /// no peer.
    Unconfigured,
}

/// The outcome of proposing a quorum round to the LOCAL criome.
enum QuorumProposalOutcome {
    /// The round opened and is `Gathering` — await its completion.
    Opened,
    /// The round is already `Authorized` (a fast or degenerate majority) and
    /// carries its assembled Evidence — ship immediately.
    AuthorizedNow(Evidence),
    /// The LOCAL criome socket could not be reached.
    Unreachable,
}

/// A detached quorum ship: everything needed to propose the captured head to the
/// LOCAL criome under the mirror quorum contract, await the round's completion,
/// and — ONLY on an `Authorized` verdict — hand the authorized head plus the
/// quorum-assembled Evidence to the LOCAL router. It owns clones of every input,
/// so the ship runs independent of the engine mailbox: a slow or unreachable peer
/// never stalls the working reply.
///
/// This is the async propose→completion boundary that REPLACES the 1-of-1 gate.
/// Withhold-until-authorized is the ship's contract: it never fabricates Evidence
/// and never hands anything to the router while the round is `Gathering` — only
/// the real quorum verdict, carrying the real assembled Evidence, releases the
/// ship. The peer's apply ingress then re-judges that same Evidence independently.
pub struct QuorumShip {
    criome_socket: PathBuf,
    contract: ContractDigest,
    object: AuthorizedObjectReference,
    versioned_entry_octets: Vec<u8>,
    origination: RouterOrigination,
    budget: QuorumCompletionBudget,
}

impl QuorumShip {
    pub fn new(
        criome_socket: PathBuf,
        contract: ContractDigest,
        object: AuthorizedObjectReference,
        versioned_entry_octets: Vec<u8>,
        origination: RouterOrigination,
        budget: QuorumCompletionBudget,
    ) -> Self {
        Self {
            criome_socket,
            contract,
            object,
            versioned_entry_octets,
            origination,
            budget,
        }
    }

    /// Run the origination boundary: propose the head under the mirror quorum
    /// contract, then either ship immediately (already a majority) or spawn a
    /// detached task that awaits the round's completion and ships on the verdict.
    /// Returns the synchronous [`QuorumOriginationOutcome`] the daemon logs.
    pub async fn originate(self) -> Result<QuorumOriginationOutcome, QuorumOriginationError> {
        match self.propose().await? {
            QuorumProposalOutcome::AuthorizedNow(evidence) => {
                self.ship(evidence).await?;
                Ok(QuorumOriginationOutcome::Shipped)
            }
            QuorumProposalOutcome::Opened => {
                tokio::spawn(self.drain());
                Ok(QuorumOriginationOutcome::Proposed)
            }
            QuorumProposalOutcome::Unreachable => Ok(QuorumOriginationOutcome::Unreachable),
        }
    }

    /// The round key, bound to the change's fingerprint (the operation digest) so
    /// the originator and the criome ingress agree on it by construction.
    fn round(&self) -> QuorumRoundIdentifier {
        QuorumRoundIdentifier::for_operation(&self.object.digest)
    }

    /// Propose the captured head under the mirror quorum contract: criome opens
    /// the round, casts this node's self-vote, and solicits the peer members
    /// across the voice, returning the withheld round state.
    async fn propose(&self) -> Result<QuorumProposalOutcome, QuorumOriginationError> {
        let proposal = QuorumProposal {
            round: self.round(),
            contract: self.contract.clone(),
            object: self.object.clone(),
            window: Self::mirror_window(),
        };
        match self
            .ask(CriomeRequest::ProposeQuorumAuthorization(proposal))
            .await?
        {
            Some(CriomeReply::QuorumRoundOpened(state)) => Ok(Self::outcome_from_state(state)),
            Some(other) => Err(QuorumOriginationError::UnexpectedReply {
                got: format!("{other:?}"),
            }),
            None => Ok(QuorumProposalOutcome::Unreachable),
        }
    }

    /// Await the round's completion on the bounded budget and, on a majority,
    /// return the quorum-assembled Evidence. `None` when the round never reaches a
    /// majority within the budget — the change is withheld, nothing ships.
    async fn await_authorized(&self) -> Result<Option<Evidence>, QuorumOriginationError> {
        let round = self.round();
        let deadline = Instant::now() + self.budget.deadline;
        loop {
            if let Some(CriomeReply::QuorumRoundObserved(state)) = self
                .ask(CriomeRequest::observe_quorum_round(round.clone()))
                .await?
                && state.status == QuorumRoundStatus::Authorized
                && let Some(evidence) = state.authorized_evidence
            {
                return Ok(Some(evidence));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(self.budget.interval).await;
        }
    }

    /// Hand the authorized head + the quorum-assembled Evidence to the LOCAL
    /// router. The submission carries byte-for-byte the head object octets, so the
    /// peer re-hashes to the same head, and the SAME Evidence the quorum
    /// authorized, so the peer's re-judge sees a true majority.
    async fn ship(self, evidence: Evidence) -> Result<(), QuorumOriginationError> {
        let payload = self
            .origination
            .submission_for_head(self.versioned_entry_octets, evidence)?;
        let origination = self.origination;
        tokio::task::spawn_blocking(move || origination.hand_off(payload))
            .await
            .map_err(|source| QuorumOriginationError::Task {
                message: source.to_string(),
            })??;
        Ok(())
    }

    /// The detached completion path: await the round's verdict and ship on a
    /// majority. On a withheld round (peer unreachable within the budget) or a
    /// transport fault, nothing ships — the change waits, never last-writer-wins.
    async fn drain(self) {
        match self.await_authorized().await {
            Ok(Some(evidence)) => {
                let _ = self.ship(evidence).await;
            }
            Ok(None) | Err(_) => {}
        }
    }

    /// One synchronous criome socket round-trip on a `spawn_blocking` worker (the
    /// `CriomeClient` transport is blocking), so the caller's task is never
    /// blocked on a slow or down criome. A socket error maps to `None` — an
    /// unreachable criome, held back fail-closed rather than surfaced as a fault.
    async fn ask(
        &self,
        request: CriomeRequest,
    ) -> Result<Option<CriomeReply>, QuorumOriginationError> {
        let socket = self.criome_socket.clone();
        let result = tokio::task::spawn_blocking(move || CriomeClient::new(socket).send(request))
            .await
            .map_err(|source| QuorumOriginationError::Task {
                message: source.to_string(),
            })?;
        Ok(result.ok())
    }

    fn outcome_from_state(state: QuorumRoundState) -> QuorumProposalOutcome {
        match (state.status, state.authorized_evidence) {
            (QuorumRoundStatus::Authorized, Some(evidence)) => {
                QuorumProposalOutcome::AuthorizedNow(evidence)
            }
            _ => QuorumProposalOutcome::Opened,
        }
    }

    /// A wide-open moment window: the mirror contract carries no time-gated rule,
    /// so the window only has to admit the round's attested moment. Both nodes
    /// time-sign the same proposition within the round.
    fn mirror_window() -> TimeWindow {
        TimeWindow {
            opens_at: TimestampNanos::new(1),
            closes_at: TimestampNanos::new(4_000_000_000_000_000_000),
        }
    }
}

/// The quorum origination boundary's typed failure modes. An unreachable criome
/// or a withheld round are NOT errors — they are [`QuorumOriginationOutcome`] /
/// `None` outcomes the daemon handles by holding the head back. An error here is a
/// real fault: a blocking task panicked, criome answered off-contract, or the
/// router hand-off projection failed.
#[derive(Debug, Error)]
pub enum QuorumOriginationError {
    #[error("quorum origination task failed: {message}")]
    Task { message: String },

    #[error("criome answered with an unexpected reply: {got}")]
    UnexpectedReply { got: String },

    #[error("router origination failed: {0}")]
    Origination(#[from] RouterOriginationError),
}
