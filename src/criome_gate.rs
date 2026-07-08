//! The criome acceptance gate (Spirit `xhwa`, corrected 2026-07-07 to the
//! everywhere-gate) — LIVE.
//!
//! **The quorum gates acceptance everywhere, including locally.** With the
//! gate `Enabled`, a head-advancing working operation is STAGED — its
//! would-be log entries are built and durably parked WITHOUT committing —
//! and this gate asks the local criome to authorize the PROSPECTIVE head
//! digest `D` the staged group would produce. Only the pushed cluster grant
//! materializes the group and releases the held accepted reply; every other
//! terminal verdict refuses the operation to the caller and the staged group
//! is discarded — the head does not advance, not even locally, and nothing
//! propagates because nothing exists. The grant is the acceptance event.
//!
//! The authorizer is the CLUSTER authorizer: spirit submits one typed
//! question over its local criome working socket ("authorize this head
//! digest", carried as the typed [`AuthorizedObjectReference`] — no contract
//! / operation / scope strings) and consumes criome's authorization
//! observation session — the submission snapshot plus pushed updates until a
//! terminal state. Criome owns everything behind that socket: quorum
//! membership, the two-round commit, windows, and the BLS material. Spirit
//! never verifies BLS locally — the socket is the trust boundary, and the
//! full cryptographic re-judgment happens where the authorization crosses a
//! real boundary (the receiving node's criome). Spirit's own checks are the
//! closed binding matrix in [`HeadSessionBinding::decide`]: slot binding,
//! digest binding, grant presence and grant binding. Every violation is a
//! [`CriomeGateError`] and every fault refuses the operation — there is no
//! default-open branch anywhere in this parse.
//!
//! Whether the seam runs at all is the gate's [`CriomeAuthorization`] policy:
//! `Disabled` (the operative default) keeps the whole seam dormant — spirit
//! fully local, heads advance freely, nothing propagates; `Enabled` carries
//! the [`ClusterAuthorizer`] (the criome socket) and demands the cluster
//! grant BEFORE any head-advancing operation is accepted. An enabled gate
//! whose criome is unreachable refuses every advance — fail-closed. Reads
//! are unaffected. The ship drain reuses the same authorizer at ship time,
//! where the ask short-circuits to criome's immediate re-grant of the
//! standing committed head (distribution, never a second acceptance
//! judgment).

#[cfg(feature = "agent-guardian")]
use criome::transport::CriomeClient;
use sema_engine::EntryDigest;

use crate::schema::signal::{AdvanceRefusalReason, Output};
use signal_criome::{
    AuthorizationRequestSlot, AuthorizationStateRecord, AuthorizationStatus, AuthorizedObjectKind,
    AuthorizedObjectReference, ComponentKind, Identity, ObjectDigest, ReplayNonce,
    SignalCallAuthorization,
};

#[cfg(feature = "agent-guardian")]
use signal_criome::SpiritAuthorizationContext;
#[cfg(feature = "agent-guardian")]
use signal_criome::SpiritProcessKey;
use thiserror::Error;

#[cfg(feature = "agent-guardian")]
const OPERATION_AUTHORIZATION_SESSION_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(300);

/// The head digest the gate authorizes — on the intake path the PROSPECTIVE
/// head the staged group would produce (nothing committed yet); on the ship
/// drain the committed head of the unshipped suffix (already accepted; the
/// re-ask short-circuits to the standing re-grant). Because the log is
/// hash-chained (each entry's digest folds in its predecessor's), this one
/// digest transitively fixes the whole ordered group beneath it — one
/// capture, one authorization, one batch. It carries the owning component so
/// ONE capture feeds both the criome request `object` and the emitted
/// reference: authorized head == materialized/fanned head by construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalHeadCapture {
    component: ComponentKind,
    head_digest: EntryDigest,
}

impl LocalHeadCapture {
    /// Capture spirit's head `D`. The component is always
    /// [`ComponentKind::Spirit`] for a spirit daemon; it is a field rather than
    /// a constant so the [`From`] projection reuses it instead of re-deciding
    /// the component at the wire boundary.
    pub fn spirit_head(head_digest: EntryDigest) -> Self {
        Self {
            component: ComponentKind::Spirit,
            head_digest,
        }
    }

    pub fn component(&self) -> ComponentKind {
        self.component
    }

    pub fn head_digest(&self) -> &EntryDigest {
        &self.head_digest
    }
}

/// The PRODUCTION projection (report 703-6: `impl From<X> for Y`, never a free
/// fn, never an inline struct literal fabricated in a test). The criome
/// `ObjectDigest` is the blake3-hex of the captured head bytes — the exact
/// shape [`ObjectDigest::from_bytes`] produces, so spirit's `EntryDigest`
/// round-trips to the same string criome attests, and the kind is always
/// [`AuthorizedObjectKind::Head`]. ONE projection feeds both the request and
/// the binding checks, so authorized digest == submitted digest by
/// construction.
impl From<&LocalHeadCapture> for AuthorizedObjectReference {
    fn from(capture: &LocalHeadCapture) -> Self {
        AuthorizedObjectReference {
            component_kind: capture.component,
            object_digest: ObjectDigest::from_bytes(capture.head_digest.bytes()),
            authorized_object_kind: AuthorizedObjectKind::Head,
        }
    }
}

/// The spirit-side criome authorization policy — a closed typed option, not a
/// flag. It decides whether spirit's heads are subject to criome authorization
/// at all.
///
/// [`Disabled`](CriomeAuthorization::Disabled) is the operative default:
/// spirit runs fully local, heads advance freely, and nothing propagates —
/// the authorize-and-ship seam stays dormant.
///
/// [`Enabled`](CriomeAuthorization::Enabled) demands the cluster grant BEFORE
/// any head-advancing working operation is accepted, and carries the
/// [`ClusterAuthorizer`] — an enabled gate always has a socket; a disabled
/// gate never runs. A refused, expired, unavailable, or unreachable verdict
/// refuses the OPERATION to the caller ([`Output::AdvanceRefused`]) and the
/// staged group is discarded: nothing is recorded anywhere, fail-closed.
/// Reads are unaffected.
///
/// The owner-only meta plane (`Import`, `CollectRemovalCandidates`) stays
/// owner-trust and is not policed by this option; `Import` is the privileged
/// escape hatch that writes locally without a round.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CriomeAuthorization {
    /// Spirit is fully local: heads advance freely, nothing propagates.
    #[default]
    Disabled,
    /// Every head advance requires cluster authorization through the carried
    /// authorizer before it ships.
    Enabled(ClusterAuthorizer),
}

/// The real cluster authorizer: spirit's side of the criome authorization
/// observation session. It holds the local criome working socket and the
/// session read deadline — the BACKSTOP for a silently dead criome process
/// (a live criome pushes its own terminal verdict, including the
/// window-close `Expired`; spirit never polls).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterAuthorizer {
    socket: std::path::PathBuf,
    session_deadline: std::time::Duration,
}

impl ClusterAuthorizer {
    /// The default session read deadline — spirit's BACKSTOP for a silently
    /// dead criome process, sized beyond the authorization window so a live
    /// criome always answers first (its own window timer pushes `Expired`)
    /// and only a dead socket trips it. Deadline expiry refuses the
    /// operation as `Unreachable` — the bound on how long a recording caller
    /// can wait before a definite verdict (§3.5.4).
    pub const DEFAULT_SESSION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

    pub fn new(socket: impl Into<std::path::PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            session_deadline: Self::DEFAULT_SESSION_DEADLINE,
        }
    }

    /// Override the dead-criome backstop deadline (tests use seconds).
    pub fn with_session_deadline(mut self, session_deadline: std::time::Duration) -> Self {
        self.session_deadline = session_deadline;
        self
    }

    pub fn socket(&self) -> &std::path::Path {
        &self.socket
    }

    /// Authorize a captured head `D` through the local criome's observation
    /// session. The synchronous `CriomeClient` stream runs on a
    /// `spawn_blocking` worker so the engine actor mailbox is never blocked
    /// while the cluster round runs.
    pub async fn authorize_head(
        &self,
        capture: &LocalHeadCapture,
    ) -> Result<GateDecision, CriomeGateError> {
        let authorizer = self.clone();
        let capture = capture.clone();
        tokio::task::spawn_blocking(move || authorizer.authorize_head_blocking(&capture))
            .await
            .map_err(|source| CriomeGateError::AuthorizationTask {
                message: source.to_string(),
            })?
    }

    /// The blocking session drive: submit the typed ask, then consume the
    /// snapshot and pushed updates until a terminal verdict, the binding
    /// matrix judging every state record. A transport failure or deadline
    /// expiry is [`GateDecision::Unreachable`]; an off-contract frame is a
    /// [`CriomeGateError`] machinery fault. On the intake path either
    /// refuses the operation; on the ship drain either holds the ship.
    fn authorize_head_blocking(
        &self,
        capture: &LocalHeadCapture,
    ) -> Result<GateDecision, CriomeGateError> {
        let submitted = AuthorizedObjectReference::from(capture);
        let authorization = self.head_authorization(submitted.clone());
        // The IO deadline is applied at connect, BEFORE the first byte moves,
        // so the SUBMISSION legs (the ask write and the snapshot read) are as
        // bounded as the later session reads — a hung-but-accepting criome
        // can never hold this blocking worker and the drain lock forever.
        let client = criome::transport::CriomeClient::new(self.socket.clone())
            .with_io_deadline(self.session_deadline);
        let mut session = match client.authorize_signal_call(authorization) {
            Ok(session) => session,
            Err(criome::Error::UnexpectedSignalFrame { got }) => {
                // criome answered off the session contract (for example the
                // retired one-shot reply shape): a machinery fault, never a
                // verdict.
                return Err(CriomeGateError::UnexpectedReply { reply: got });
            }
            Err(_unreachable) => return Ok(GateDecision::Unreachable),
        };
        let binding = HeadSessionBinding::new(session.token().payload().clone(), submitted);
        // The fast path: the submission snapshot may already carry the
        // terminal state (the degenerate immediate grant); pending-then-pushed
        // is the normal case, and both run through the same matrix.
        for state in session.snapshot().states() {
            if let Some(decision) = binding.decide(state)? {
                return Ok(decision);
            }
        }
        loop {
            let state = match session.next_update() {
                Ok(state) => state,
                Err(criome::Error::UnexpectedSignalFrame { got }) => {
                    return Err(CriomeGateError::UnexpectedReply { reply: got });
                }
                // A read deadline expiry or a dead socket: criome cannot push
                // anything (its own window timer would otherwise have pushed
                // Expired), so the head is held Unreachable.
                Err(_dead) => return Ok(GateDecision::Unreachable),
            };
            if let Some(decision) = binding.decide(&state)? {
                return Ok(decision);
            }
        }
    }

    /// The typed head ask: exactly one question with no quorum vocabulary —
    /// "authorize this head digest". No contract-name / operation / scope
    /// strings, no Evidence, no window: policy is criome's.
    fn head_authorization(&self, submitted: AuthorizedObjectReference) -> SignalCallAuthorization {
        SignalCallAuthorization::new(
            submitted,
            Identity::host("spirit".to_owned()),
            self.replay_nonce(),
            None,
        )
    }

    fn replay_nonce(&self) -> ReplayNonce {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        ReplayNonce::new(format!("spirit-head-{nanos}"))
    }
}

/// One head-authorization session's binding facts — the request slot criome
/// assigned to THIS submission and the submitted typed head reference — and
/// the CLOSED parse matrix every observed state record runs through. This is
/// the security-sensitive contact point (`AuthorizationStatus` ×
/// grant-presence → decision), written once:
///
///   1. Slot binding: only records whose `request_slot` equals the session
///      token are considered; a foreign record is ignored, never trusted.
///   2. Digest binding: a token-bound record whose `request_digest` differs
///      from the submitted head digest is a machinery fault.
///   3. Terminal `Granted` requires the grant, and the grant must bind the
///      slot and the submitted digest — status alone is never proof.
///   4. Terminal `Denied` / `Expired` / `Unavailable` are typed refusals
///      (outcomes, not errors): on the intake path the OPERATION is refused
///      to the caller and the staged group discarded; on the ship drain the
///      ship is withheld.
///   5. Non-terminal `Pending` / `Signing` / `Parked` keep the session
///      draining pushed updates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadSessionBinding {
    token_slot: AuthorizationRequestSlot,
    submitted: AuthorizedObjectReference,
}

impl HeadSessionBinding {
    pub fn new(token_slot: AuthorizationRequestSlot, submitted: AuthorizedObjectReference) -> Self {
        Self {
            token_slot,
            submitted,
        }
    }

    /// Judge one observed state record: `Ok(Some(decision))` is a terminal
    /// gate decision, `Ok(None)` keeps draining (a foreign or non-terminal
    /// record), and `Err` is a machinery fault that refuses the operation.
    pub fn decide(
        &self,
        state: &AuthorizationStateRecord,
    ) -> Result<Option<GateDecision>, CriomeGateError> {
        // Rule 1 — slot binding. Foreign records are ignored, never judged.
        if state.authorization_request_slot != self.token_slot {
            return Ok(None);
        }
        // Rule 2 — digest binding.
        if state.object_digest != self.submitted.object_digest {
            return Err(CriomeGateError::HeadBindingViolation {
                detail: format!(
                    "state record for slot {} carries digest {} instead of the submitted {}",
                    self.token_slot.as_str(),
                    state.object_digest.as_str(),
                    self.submitted.object_digest.as_str()
                ),
            });
        }
        match (
            state.authorization_status,
            state.optional_authorization_grant(),
        ) {
            // Rule 3 — Granted requires the binding grant.
            (AuthorizationStatus::Granted, Some(grant))
                if grant.authorization_request_slot == self.token_slot
                    && grant.authorized_object_digest() == &self.submitted.object_digest =>
            {
                Ok(Some(GateDecision::Authorized(self.submitted.clone())))
            }
            (AuthorizationStatus::Granted, _absent_or_mismatched) => {
                Err(CriomeGateError::HeadBindingViolation {
                    detail: format!(
                        "a Granted state for slot {} carries no grant binding the submitted \
                         digest {} — status alone is never proof",
                        self.token_slot.as_str(),
                        self.submitted.object_digest.as_str()
                    ),
                })
            }
            // Rule 4 — terminal refusals are outcomes, not errors.
            (AuthorizationStatus::Denied, _) => {
                Ok(Some(GateDecision::Refused(GateRefusal::Denied)))
            }
            (AuthorizationStatus::Expired, _) => {
                Ok(Some(GateDecision::Refused(GateRefusal::Expired)))
            }
            (AuthorizationStatus::Unavailable, _) => {
                Ok(Some(GateDecision::Refused(GateRefusal::Unavailable)))
            }
            // Rule 5 — non-terminal states keep the session draining.
            (
                AuthorizationStatus::Pending
                | AuthorizationStatus::Signing
                | AuthorizationStatus::Parked,
                _,
            ) => Ok(None),
        }
    }
}

/// The terminal decision one authorization session produces. On the INTAKE
/// path (§3.5) only [`GateDecision::Authorized`] materializes the staged
/// group and releases the held accepted reply; every other decision refuses
/// the operation to the caller and the staged group is discarded — nothing
/// recorded anywhere. On the SHIP drain (§3.6) only `Authorized` releases the
/// ship; every other decision leaves the (already accepted) suffix waiting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateDecision {
    /// criome's cluster authorized head `D`. Carries the projected reference
    /// so the caller materializes or ships exactly what was authorized.
    Authorized(AuthorizedObjectReference),
    /// criome reached a terminal verdict but did not authorize. Intake:
    /// the operation is refused ([`Output::AdvanceRefused`]) and the staged
    /// group discarded — its digest names entries that will never exist, and
    /// criome's dead-round supersession admits a later differing successor.
    /// Ship drain: the suffix waits for the next mail.
    Refused(GateRefusal),
    /// criome was not reachable (no socket, dead process, session deadline).
    /// Intake: the operation is refused. Ship drain: the suffix waits.
    Unreachable,
}

/// The typed refusal a terminal non-Granted state maps to — an outcome, never
/// an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateRefusal {
    /// criome denied the head advance.
    Denied,
    /// The authorization window closed before the quorum completed —
    /// fail-closed.
    Expired,
    /// No operational quorum contract is available (an unfounded criome
    /// refusing loudly).
    Unavailable,
}

impl GateDecision {
    /// Whether this decision releases the fan-out.
    pub fn ships(&self) -> bool {
        matches!(self, GateDecision::Authorized(_))
    }

    /// The closed [`GateDecision`] → [`AdvanceRefusalReason`] contact point
    /// (§3.5.2): the reason the caller receives when this decision refuses
    /// the operation. `None` exactly when the decision is the grant.
    pub fn refusal_reason(&self) -> Option<AdvanceRefusalReason> {
        match self {
            GateDecision::Authorized(_reference) => None,
            GateDecision::Refused(refusal) => Some(AdvanceRefusalReason::from(*refusal)),
            GateDecision::Unreachable => Some(AdvanceRefusalReason::Unreachable),
        }
    }
}

/// The wire projection of a [`GateRefusal`]: the two contact points keep
/// their own closed types, mapped once here.
impl From<GateRefusal> for AdvanceRefusalReason {
    fn from(refusal: GateRefusal) -> Self {
        match refusal {
            GateRefusal::Denied => AdvanceRefusalReason::Denied,
            GateRefusal::Expired => AdvanceRefusalReason::Expired,
            GateRefusal::Unavailable => AdvanceRefusalReason::Unavailable,
        }
    }
}

/// One staged head advance in flight (§3.5): the parked group's prospective
/// head digest, the authorizer to run the cluster round against, the held
/// accepted reply, and — after [`StagedHeadAdvance::resolve`] — the terminal
/// verdict. It crosses the daemon spine as the emitted `StagedAdvance`
/// object: `resolve` runs on the connection task with no engine borrow (the
/// quorum wait), and the engine's concluding turn materializes on the grant
/// or discards on any other verdict.
#[derive(Debug)]
pub struct StagedHeadAdvance {
    authorizer: ClusterAuthorizer,
    prospective_head: EntryDigest,
    held_reply: Output,
    verdict: Option<GateDecision>,
}

impl StagedHeadAdvance {
    pub fn new(
        authorizer: ClusterAuthorizer,
        prospective_head: EntryDigest,
        held_reply: Output,
    ) -> Self {
        Self {
            authorizer,
            prospective_head,
            held_reply,
            verdict: None,
        }
    }

    /// The quorum wait: authorize the prospective head through the cluster
    /// session and store the terminal verdict. A [`CriomeGateError`]
    /// machinery fault also refuses the operation — the caller cannot
    /// distinguish machinery from refusal and should not; the fault is
    /// logged loudly on the daemon side and judged `Unreachable`.
    pub async fn resolve(&mut self) {
        let capture = LocalHeadCapture::spirit_head(self.prospective_head.clone());
        let verdict = match self.authorizer.authorize_head(&capture).await {
            Ok(decision) => decision,
            Err(machinery_fault) => {
                eprintln!(
                    "spirit staged advance {} refused on gate machinery fault: {machinery_fault}",
                    self.prospective_head
                );
                GateDecision::Unreachable
            }
        };
        self.verdict = Some(verdict);
    }

    /// The stored terminal verdict — `None` until [`Self::resolve`] ran
    /// (an unresolved advance concludes as `Unreachable`, fail-closed).
    pub fn verdict(&self) -> Option<&GateDecision> {
        self.verdict.as_ref()
    }

    /// The prospective head digest the parked group would produce.
    pub fn prospective_head(&self) -> &EntryDigest {
        &self.prospective_head
    }

    /// Release the held accepted reply (the grant arrived and the group
    /// materialized — acceptance happened at the grant).
    pub fn into_held_reply(self) -> Output {
        self.held_reply
    }
}

/// The criome head-authorization gate: the policy holder the engine composes.
/// `Disabled` keeps the seam dormant; `Enabled` carries the live
/// [`ClusterAuthorizer`].
#[derive(Debug, Default)]
pub struct CriomeGate {
    authorization: CriomeAuthorization,
}

impl CriomeGate {
    /// The default gate: criome authorization [`CriomeAuthorization::Disabled`]
    /// — spirit fully local, the authorize-and-ship seam dormant.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the spirit-side criome authorization policy.
    pub fn set_authorization(&mut self, authorization: CriomeAuthorization) {
        self.authorization = authorization;
    }

    /// The gate's current criome authorization policy.
    pub fn authorization(&self) -> &CriomeAuthorization {
        &self.authorization
    }
}

#[cfg(feature = "agent-guardian")]
#[derive(Clone, Debug)]
pub struct SpiritOperationAuthorizer {
    socket: Option<std::path::PathBuf>,
    process_key: SpiritProcessKey,
}

#[cfg(feature = "agent-guardian")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpiritOperationAuthorization {
    Allowed,
    Blocked(String),
}

#[cfg(feature = "agent-guardian")]
impl SpiritOperationAuthorizer {
    pub fn new() -> Self {
        Self {
            socket: None,
            process_key: SpiritProcessKey::new("spirit-process-main"),
        }
    }

    pub fn configure_socket(&mut self, socket: impl Into<std::path::PathBuf>) {
        self.socket = Some(socket.into());
    }

    pub fn clear(&mut self) {
        self.socket = None;
    }

    pub fn process_key(&self) -> SpiritProcessKey {
        self.process_key.clone()
    }

    /// Authorize one guardian-observed spirit operation through the local
    /// criome's observation session (the same push-based session contract the
    /// head gate consumes — the earlier one-shot send plus 100 ms
    /// re-observation poll is retired). `Observing` mode validates the
    /// submission and returns without waiting for a terminal state; `Gating`
    /// mode drains pushed updates until the terminal verdict or the session
    /// deadline.
    pub async fn authorize(
        &self,
        context: SpiritAuthorizationContext,
        mode: signal_spirit::AuthorizationMode,
    ) -> Result<SpiritOperationAuthorization, CriomeGateError> {
        let Some(socket) = self.socket.clone() else {
            return Ok(SpiritOperationAuthorization::Allowed);
        };
        let authorizer = self.clone();
        tokio::task::spawn_blocking(move || authorizer.authorize_blocking(socket, context, mode))
            .await
            .map_err(|source| CriomeGateError::AuthorizationTask {
                message: source.to_string(),
            })?
    }

    fn authorize_blocking(
        &self,
        socket: std::path::PathBuf,
        context: SpiritAuthorizationContext,
        mode: signal_spirit::AuthorizationMode,
    ) -> Result<SpiritOperationAuthorization, CriomeGateError> {
        let request_digest =
            ObjectDigest::from_bytes(context.raw_spirit_operation_payload.payload().as_bytes());
        let submitted = AuthorizedObjectReference {
            component_kind: ComponentKind::Spirit,
            object_digest: request_digest,
            authorized_object_kind: AuthorizedObjectKind::Operation,
        };
        let authorization = self.signal_call_authorization(context, submitted.clone());
        // The IO deadline bounds the SUBMISSION legs too (connect-adjacent
        // write and snapshot read) — including Observing mode, whose whole
        // check IS the submission and which never reaches a later
        // set_read_timeout.
        let mut session = match CriomeClient::new(socket)
            .with_io_deadline(OPERATION_AUTHORIZATION_SESSION_DEADLINE)
            .authorize_signal_call(authorization)
        {
            Ok(session) => session,
            Err(criome::Error::UnexpectedSignalFrame { got }) => {
                return Err(CriomeGateError::UnexpectedReply { reply: got });
            }
            Err(_unreachable) => {
                return Ok(SpiritOperationAuthorization::Blocked(
                    "criome operation authorization unreachable".to_owned(),
                ));
            }
        };
        let binding = HeadSessionBinding::new(session.token().payload().clone(), submitted.clone());
        if mode == signal_spirit::AuthorizationMode::Observing {
            // Observing never gates: the submission itself (an on-contract
            // session with a digest-bound state) is the whole check.
            for state in session.snapshot().states() {
                let _decision = binding.decide(state)?;
            }
            return Ok(SpiritOperationAuthorization::Allowed);
        }
        for state in session.snapshot().states() {
            if let Some(decision) = binding.decide(state)? {
                return Ok(Self::operation_authorization(decision));
            }
        }
        loop {
            let state = match session.next_update() {
                Ok(state) => state,
                Err(criome::Error::UnexpectedSignalFrame { got }) => {
                    return Err(CriomeGateError::UnexpectedReply { reply: got });
                }
                Err(_dead) => {
                    return Ok(SpiritOperationAuthorization::Blocked(format!(
                        "criome operation authorization timed out waiting for request {}",
                        binding.token_slot.as_str()
                    )));
                }
            };
            if let Some(decision) = binding.decide(&state)? {
                return Ok(Self::operation_authorization(decision));
            }
        }
    }

    fn operation_authorization(decision: GateDecision) -> SpiritOperationAuthorization {
        match decision {
            GateDecision::Authorized(_reference) => SpiritOperationAuthorization::Allowed,
            GateDecision::Refused(refusal) => SpiritOperationAuthorization::Blocked(format!(
                "criome operation authorization refused: {refusal:?}"
            )),
            GateDecision::Unreachable => SpiritOperationAuthorization::Blocked(
                "criome operation authorization unreachable while waiting".to_owned(),
            ),
        }
    }

    fn signal_call_authorization(
        &self,
        context: SpiritAuthorizationContext,
        submitted: AuthorizedObjectReference,
    ) -> SignalCallAuthorization {
        SignalCallAuthorization::new(
            submitted,
            Identity::host("spirit".to_owned()),
            self.replay_nonce(),
            None,
        )
        .with_spirit_context(context)
    }

    fn replay_nonce(&self) -> ReplayNonce {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        ReplayNonce::new(format!("spirit-operation-{nanos}"))
    }
}

#[cfg(feature = "agent-guardian")]
impl Default for SpiritOperationAuthorizer {
    fn default() -> Self {
        Self::new()
    }
}

/// The gate's typed failure modes. A REFUSED or UNREACHABLE criome is NOT an
/// error — those are [`GateDecision`] outcomes the drain handles by holding
/// the head back. An error here is a real fault in the gate's own machinery:
/// the blocking task panicked/cancelled, criome answered off the session
/// contract, or a session record violated the binding matrix.
#[derive(Debug, Error)]
pub enum CriomeGateError {
    #[error("criome authorization task failed: {message}")]
    AuthorizationTask { message: String },

    #[error("criome answered with an unexpected reply: {reply}")]
    UnexpectedReply { reply: String },

    #[error("criome authorization session violated the head binding: {detail}")]
    HeadBindingViolation { detail: String },
}
