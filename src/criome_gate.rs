//! The criome head-authorization seam (Spirit `xhwa`) — LIVE.
//!
//! A spirit working commit lands LOCALLY first (the durable SEMA log write in
//! `Engine::handle_async`). Before that committed head fans out, the
//! propagation drain asks this gate to authorize the content-addressed head
//! `D` — the head of the WHOLE unshipped suffix (one authorization covers the
//! hash-chained batch) — and ships only on an explicit
//! [`GateDecision::Authorized`].
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
//! closed binding matrix in [`HeadSessionBinding::verdict`]: slot binding,
//! digest binding, grant presence and grant binding. Every violation is a
//! [`CriomeGateError`] and every fault holds the head — there is no
//! default-open branch anywhere in this parse.
//!
//! Whether the seam runs at all is the gate's [`CriomeAuthorization`] policy:
//! `Disabled` (the operative default) keeps the whole authorize-and-ship seam
//! dormant — heads advance freely and nothing propagates; `Enabled` carries
//! the [`ClusterAuthorizer`] (the criome socket) and demands cluster
//! authorization for every head advance. An enabled gate whose criome is
//! unreachable holds every head back — the local commit stands, the outbox
//! waits, fail-closed.

#[cfg(feature = "agent-guardian")]
use criome::transport::CriomeClient;
use sema_engine::EntryDigest;
use signal_criome::{
    AuthorizationRequestSlot, AuthorizationStateRecord, AuthorizationStatus,
    AuthorizedObjectKind, AuthorizedObjectReference, ComponentKind, Identity, ObjectDigest,
    ReplayNonce, SignalCallAuthorization,
};

#[cfg(feature = "agent-guardian")]
use signal_criome::SpiritAuthorizationContext;
#[cfg(feature = "agent-guardian")]
use signal_criome::SpiritProcessKey;
use thiserror::Error;

#[cfg(feature = "agent-guardian")]
const OPERATION_AUTHORIZATION_SESSION_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(300);

/// The post-commit local head the gate authorizes BEFORE fan-out — the
/// content-addressed identity `D` of the latest versioned-log entry, captured
/// from the LOCAL log (never from `ShipOutcome.head`, which exists only after a
/// ship). Because the log is hash-chained (each entry's digest folds in its
/// predecessor's), this one head transitively fixes the whole unshipped
/// suffix beneath it — one capture, one authorization, one batch. It carries
/// the owning component so ONE capture feeds both the criome request `object`
/// and the emitted reference: authorized head == fanned head by construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalHeadCapture {
    component: ComponentKind,
    head_digest: EntryDigest,
}

impl LocalHeadCapture {
    /// Capture spirit's post-commit head `D`. The component is always
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
            component: capture.component,
            digest: ObjectDigest::from_bytes(capture.head_digest.bytes()),
            kind: AuthorizedObjectKind::Head,
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
/// [`Enabled`](CriomeAuthorization::Enabled) demands cluster authorization for
/// every head advance and carries the [`ClusterAuthorizer`] — an enabled gate
/// always has a socket; a disabled gate never runs. Working inputs are NOT
/// refused at ingress: the local commit stands, and only propagation waits on
/// the cluster verdict.
///
/// The owner-only meta plane (`Import`, `CollectRemovalCandidates`) stays
/// owner-trust and is not policed by this option.
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
    /// The default session read deadline: sized beyond the authorization
    /// window's catch-up case (which chains two commit rounds), so a live
    /// criome always answers first and only a dead criome process trips it.
    pub const DEFAULT_SESSION_DEADLINE: std::time::Duration =
        std::time::Duration::from_secs(120);

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
    /// expiry is [`GateDecision::Unreachable`] (head held); an off-contract
    /// frame is a [`CriomeGateError`] machinery fault (head held).
    fn authorize_head_blocking(
        &self,
        capture: &LocalHeadCapture,
    ) -> Result<GateDecision, CriomeGateError> {
        let submitted = AuthorizedObjectReference::from(capture);
        let authorization = self.head_authorization(submitted.clone());
        let client = criome::transport::CriomeClient::new(self.socket.clone());
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
        if session.set_read_timeout(Some(self.session_deadline)).is_err() {
            return Ok(GateDecision::Unreachable);
        }
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
///      (outcomes, not errors): head held, outbox waits, the next drain
///      re-asks.
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
    /// record), and `Err` is a machinery fault that holds the head.
    pub fn decide(
        &self,
        state: &AuthorizationStateRecord,
    ) -> Result<Option<GateDecision>, CriomeGateError> {
        // Rule 1 — slot binding. Foreign records are ignored, never judged.
        if state.request_slot != self.token_slot {
            return Ok(None);
        }
        // Rule 2 — digest binding.
        if state.request_digest != self.submitted.digest {
            return Err(CriomeGateError::HeadBindingViolation {
                detail: format!(
                    "state record for slot {} carries digest {} instead of the submitted {}",
                    self.token_slot.as_str(),
                    state.request_digest.as_str(),
                    self.submitted.digest.as_str()
                ),
            });
        }
        match (state.status, state.grant()) {
            // Rule 3 — Granted requires the binding grant.
            (AuthorizationStatus::Granted, Some(grant))
                if grant.request_slot == self.token_slot
                    && grant.authorized_object_digest() == &self.submitted.digest =>
            {
                Ok(Some(GateDecision::Authorized(self.submitted.clone())))
            }
            (AuthorizationStatus::Granted, _absent_or_mismatched) => {
                Err(CriomeGateError::HeadBindingViolation {
                    detail: format!(
                        "a Granted state for slot {} carries no grant binding the submitted \
                         digest {} — status alone is never proof",
                        self.token_slot.as_str(),
                        self.submitted.digest.as_str()
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

/// The decision the gate returns to the propagation drain. Only
/// [`GateDecision::Authorized`] releases the ship; every other decision holds
/// the head back — the local commit stands and the suffix waits for the next
/// drain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateDecision {
    /// criome's cluster authorized head `D`. Carries the projected reference
    /// so the drain ships exactly the suffix it authorized.
    Authorized(AuthorizedObjectReference),
    /// criome reached a terminal verdict but did not authorize. Do not ship;
    /// the outbox waits and the next drain re-asks with the then-current head
    /// (the criome-side catch-up rule makes that safe even when the head
    /// moved).
    Refused(GateRefusal),
    /// criome was not reachable (no socket, dead process, session deadline).
    /// Do not ship; the local commit stands and the suffix waits.
    Unreachable,
}

/// The typed refusal a terminal non-Granted state maps to — an outcome the
/// drain handles by holding the head, never an error.
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
        tokio::task::spawn_blocking(move || {
            authorizer.authorize_blocking(socket, context, mode)
        })
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
        let request_digest = ObjectDigest::from_bytes(context.raw_payload.payload().as_bytes());
        let submitted = AuthorizedObjectReference {
            component: ComponentKind::Spirit,
            digest: request_digest,
            kind: AuthorizedObjectKind::Operation,
        };
        let authorization = self.signal_call_authorization(context, submitted.clone());
        let mut session = match CriomeClient::new(socket).authorize_signal_call(authorization) {
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
        let binding =
            HeadSessionBinding::new(session.token().payload().clone(), submitted.clone());
        if mode == signal_spirit::AuthorizationMode::Observing {
            // Observing never gates: the submission itself (an on-contract
            // session with a digest-bound state) is the whole check.
            for state in session.snapshot().states() {
                let _decision = binding.decide(state)?;
            }
            return Ok(SpiritOperationAuthorization::Allowed);
        }
        if session
            .set_read_timeout(Some(OPERATION_AUTHORIZATION_SESSION_DEADLINE))
            .is_err()
        {
            return Ok(SpiritOperationAuthorization::Blocked(
                "criome operation authorization unreachable".to_owned(),
            ));
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
