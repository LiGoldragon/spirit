//! The 1-of-1 LOCAL criome gate (Spirit `xhwa`).
//!
//! A spirit working commit lands LOCALLY first (the durable SEMA log write in
//! `Engine::handle_async`). Before that committed head fans out to the mirror,
//! the daemon asks its **co-resident local** criome daemon to authorize the
//! content-addressed head `D`. A single local criome authorization (1-of-1, no
//! quorum, no multi-machine cluster) suffices to gate propagation: criome's
//! `k > n/2` rule already admits `n=1, k=1`, so the 1-member root is a criome
//! DEPLOY-CONFIG concern, not spirit code.
//!
//! The gate inverts today's best-effort ship. Today an unreachable mirror is
//! logged and the suffix waits; here an unconfigured, denied, or unreachable
//! criome means the head does NOT fan out — the local commit stands, the outbox
//! waits, and the next authorized drain ships it. Only an explicit `Authorized`
//! decision over a real socket round-trip releases the fan-out.
//!
//! Posture (designer lean, report 703-6 Item 1): the gate calls the LOCAL
//! criome over the per-user Unix socket via [`criome::transport::CriomeClient`].
//! `CriomeClient::send` is SYNCHRONOUS (`UnixStream::connect`), and the daemon's
//! `handle_working_input` is async on the actor mailbox, so every socket call is
//! wrapped in `tokio::task::spawn_blocking` — the mailbox is never blocked on a
//! down or slow criome.

use criome::transport::CriomeClient;
use sema_engine::EntryDigest;
use signal_criome::{
    AttestedMoment, AttestedMomentProposition, AuthorizationEvaluation, AuthorizationScope,
    AuthorizedObjectKind, AuthorizedObjectReference, ComponentKind, ContractDigest, ContractName,
    ContractOperationHead, CriomeReply, CriomeRequest, EvaluationDecision, Evidence, Identity,
    ObjectDigest, OperationDigest, ReplayNonce, RequiredSignatureThreshold,
    SignalCallAuthorization, TimeWindow, TimestampNanos,
};
use thiserror::Error;

/// The post-commit local head the gate authorizes BEFORE fan-out — the
/// content-addressed identity `D` of the latest versioned-log entry, captured
/// from the LOCAL log (never from `ShipOutcome.head`, which exists only after a
/// ship). It carries the owning component so ONE capture feeds both the
/// criome request `object` and the emitted reference: authorized head == fanned
/// head by construction.
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
/// [`AuthorizedObjectKind::Head`].
impl From<&LocalHeadCapture> for AuthorizedObjectReference {
    fn from(capture: &LocalHeadCapture) -> Self {
        AuthorizedObjectReference {
            component: capture.component,
            digest: ObjectDigest::from_bytes(capture.head_digest.bytes()),
            kind: AuthorizedObjectKind::Head,
        }
    }
}

/// The deploy-config trust material for the 1-of-1 gate.
///
/// A configured policy attestor evaluates a head against an admitted criome
/// contract. The socket-only bootstrap attestor submits a `SignalCallAuthorization`
/// and requires criome to return a signed `AuthorizationGrant`; the request may
/// be simple, but approval is still criome's signed answer.
#[derive(Clone, Debug)]
pub struct SpiritAttestor {
    contract: Option<ContractDigest>,
    evidence: Option<Evidence>,
}

impl SpiritAttestor {
    /// Build the attestor from the admitted contract digest and the signed
    /// evidence over the head's operation digest.
    pub fn new(contract: ContractDigest, evidence: Evidence) -> Self {
        Self {
            contract: Some(contract),
            evidence: Some(evidence),
        }
    }

    /// Build the bootstrap attestor used when owner meta Configure supplies a
    /// local criome socket but no quorum signer material.
    pub fn unsigned() -> Self {
        Self {
            contract: None,
            evidence: None,
        }
    }

    pub fn contract(&self, capture: &LocalHeadCapture) -> ContractDigest {
        self.contract
            .clone()
            .unwrap_or_else(|| ContractDigest::from_bytes(capture.head_digest.bytes()))
    }

    fn has_policy_evidence(&self) -> bool {
        self.evidence.is_some()
    }

    /// The full [`AuthorizationEvaluation`] for a captured head: the projected
    /// object reference, the admitted contract, and the signed evidence. ONE
    /// projection feeds the request `object`, so the authorized head and the
    /// fanned head are the same digest.
    pub fn evaluation(&self, capture: &LocalHeadCapture) -> AuthorizationEvaluation {
        AuthorizationEvaluation {
            contract: self.contract(capture),
            object: AuthorizedObjectReference::from(capture),
            evidence: self
                .evidence
                .clone()
                .unwrap_or_else(|| UnsignedHeadEvidence::from(capture).into_evidence()),
        }
    }

    fn signal_call_authorization(&self, capture: &LocalHeadCapture) -> SignalCallAuthorization {
        let digest = ObjectDigest::from_bytes(capture.head_digest.bytes());
        SignalCallAuthorization::new(
            digest.clone(),
            ContractName::new("spirit-local-head"),
            ContractOperationHead::new("AuthorizeHead"),
            AuthorizationScope::new("spirit-head-fanout"),
            Identity::host("spirit".to_owned()),
            ReplayNonce::new(digest.as_str().to_owned()),
            None,
        )
    }
}

struct UnsignedHeadEvidence<'capture> {
    capture: &'capture LocalHeadCapture,
}

impl<'capture> From<&'capture LocalHeadCapture> for UnsignedHeadEvidence<'capture> {
    fn from(capture: &'capture LocalHeadCapture) -> Self {
        Self { capture }
    }
}

impl UnsignedHeadEvidence<'_> {
    fn into_evidence(self) -> Evidence {
        Evidence::new(
            self.capture.component,
            OperationDigest::from_bytes(self.capture.head_digest.bytes()),
            self.attested_moment(),
            Vec::new(),
            Vec::new(),
        )
    }

    fn attested_moment(&self) -> AttestedMoment {
        AttestedMoment::new(
            AttestedMomentProposition::new(
                TimeWindow {
                    opens_at: TimestampNanos::new(0),
                    closes_at: TimestampNanos::new(u64::MAX),
                },
                RequiredSignatureThreshold::new(0),
                Vec::new(),
            ),
            Vec::new(),
        )
    }
}

/// The decision the gate returns to the daemon's fan-out point. Gating mode
/// releases the ship only on [`GateDecision::Authorized`]. Observing mode
/// releases on [`GateDecision::Observed`]: the request completed against
/// criome and spirit records what would have blocked in gating mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateDecision {
    /// criome authorized head `D` over a real socket round-trip. Carries the
    /// projected reference so the daemon emits the SAME reference it authorized.
    Authorized(AuthorizedObjectReference),
    /// The observe-only path received criome's verdict and proceeded without
    /// applying it as a blocking gate.
    Observed(ObservedAuthorization),
    /// No local criome socket + attestor are configured. Do not ship: this is
    /// a missing authorization gate, not permission to fan out.
    Unconfigured,
    /// criome reached a decision but did not authorize (rejected quorum/time/
    /// signature, or escalated to the psyche). Do not ship.
    Denied(EvaluationDecision),
    /// criome was not reachable over the socket. Do not ship; the local commit
    /// stands and the suffix waits for the next drain.
    Unreachable,
}

impl GateDecision {
    /// Whether this decision releases the fan-out.
    pub fn ships(&self) -> bool {
        matches!(
            self,
            GateDecision::Authorized(_) | GateDecision::Observed(_)
        )
    }
}

/// The authorization result observed in non-blocking mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedAuthorization {
    reference: AuthorizedObjectReference,
    decision: EvaluationDecision,
}

impl ObservedAuthorization {
    pub fn new(reference: AuthorizedObjectReference, decision: EvaluationDecision) -> Self {
        Self {
            reference,
            decision,
        }
    }

    pub fn reference(&self) -> &AuthorizedObjectReference {
        &self.reference
    }

    pub fn decision(&self) -> &EvaluationDecision {
        &self.decision
    }

    pub fn authorized(&self) -> bool {
        self.decision == EvaluationDecision::Authorized
    }
}

enum AuthorizationObservation {
    Observed(ObservedAuthorization),
    Unconfigured,
    Unreachable,
}

/// The 1-of-1 LOCAL criome gate. `Off` is the fail-closed default and the only
/// state a daemon reaches without an owner-configured criome socket + attestor:
/// it holds the head back because no authorization exists. `Armed` holds the
/// local socket client and the deploy-config attestor, and gates every fan-out
/// behind a real criome round-trip.
#[derive(Default)]
pub struct CriomeGate {
    armed: Option<ArmedGate>,
}

struct ArmedGate {
    socket: std::path::PathBuf,
    attestor: SpiritAttestor,
}

impl CriomeGate {
    /// An unarmed gate: no local criome configured, so fan-out does not ship.
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm the gate against a local criome socket path and the deploy-config
    /// attestor. Re-arming replaces the prior configuration.
    pub fn arm(&mut self, socket: impl Into<std::path::PathBuf>, attestor: SpiritAttestor) {
        self.armed = Some(ArmedGate {
            socket: socket.into(),
            attestor,
        });
    }

    /// Configure the gate from owner meta policy with only a local criome
    /// socket. The submitted request is simple bootstrap input; authorization
    /// is complete only when criome returns its signed grant.
    pub fn configure_socket(&mut self, socket: impl Into<std::path::PathBuf>) {
        self.arm(socket, SpiritAttestor::unsigned());
    }

    /// Clear the configured local criome gate.
    pub fn clear(&mut self) {
        self.armed = None;
    }

    /// Whether a local criome gate is configured and live.
    pub fn is_armed(&self) -> bool {
        self.armed.is_some()
    }

    /// The configured local criome socket path, when armed.
    pub fn socket_path(&self) -> Option<&std::path::Path> {
        self.armed.as_ref().map(|armed| armed.socket.as_path())
    }

    /// Authorize a captured head `D` over the LOCAL criome socket. Returns
    /// [`GateDecision::Authorized`] (with the projected reference) only on an
    /// explicit `Authorized` decision. An unconfigured gate is
    /// [`GateDecision::Unconfigured`], a reached-but-not-authorized decision is
    /// [`GateDecision::Denied`], and an unreachable socket is
    /// [`GateDecision::Unreachable`] — all hold the head back.
    ///
    /// When the gate is unarmed it returns [`GateDecision::Unconfigured`]:
    /// missing local criome configuration is a missing authorization gate, not
    /// permission to ship.
    ///
    /// `CriomeClient::send` is synchronous, so the socket round-trip runs on a
    /// `spawn_blocking` worker: the actor mailbox driving `handle_working_input`
    /// is never blocked on a slow or down criome.
    pub async fn authorize_head(
        &self,
        capture: &LocalHeadCapture,
    ) -> Result<GateDecision, CriomeGateError> {
        Ok(match self.evaluate_authorization(capture).await? {
            AuthorizationObservation::Observed(observed) => match observed.decision {
                EvaluationDecision::Authorized => GateDecision::Authorized(observed.reference),
                other => GateDecision::Denied(other),
            },
            AuthorizationObservation::Unconfigured => GateDecision::Unconfigured,
            AuthorizationObservation::Unreachable => GateDecision::Unreachable,
        })
    }

    /// Observe a captured head authorization request against the LOCAL criome
    /// socket and release fan-out without applying the verdict as a gate.
    ///
    /// This is the trace-scaffold mode: criome receives the same
    /// `EvaluateAuthorization` payload it receives under gating, spirit waits
    /// long enough to know what criome answered, and later tracing can show
    /// where a blocking gate would have released or held the head.
    pub async fn observe_authorization(
        &self,
        capture: &LocalHeadCapture,
    ) -> Result<GateDecision, CriomeGateError> {
        Ok(match self.evaluate_authorization(capture).await? {
            AuthorizationObservation::Observed(observed) => GateDecision::Observed(observed),
            AuthorizationObservation::Unconfigured => GateDecision::Unconfigured,
            AuthorizationObservation::Unreachable => GateDecision::Unreachable,
        })
    }

    async fn evaluate_authorization(
        &self,
        capture: &LocalHeadCapture,
    ) -> Result<AuthorizationObservation, CriomeGateError> {
        let reference = AuthorizedObjectReference::from(capture);
        let Some(armed) = self.armed.as_ref() else {
            return Ok(AuthorizationObservation::Unconfigured);
        };
        let socket = armed.socket.clone();
        if !armed.attestor.has_policy_evidence() {
            let authorization = armed.attestor.signal_call_authorization(capture);
            return self
                .authorize_signal_call(socket, authorization, reference)
                .await;
        }
        let evaluation = armed.attestor.evaluation(capture);
        let send_result = tokio::task::spawn_blocking(move || {
            CriomeClient::new(socket).send(CriomeRequest::EvaluateAuthorization(evaluation))
        })
        .await
        .map_err(|source| CriomeGateError::AuthorizationTask {
            message: source.to_string(),
        })?;
        let reply = match send_result {
            Ok(reply) => reply,
            Err(_) => return Ok(AuthorizationObservation::Unreachable),
        };
        let CriomeReply::AuthorizationEvaluated(evaluated) = reply else {
            return Err(CriomeGateError::UnexpectedReply {
                reply: format!("{reply:?}"),
            });
        };
        Ok(AuthorizationObservation::Observed(
            ObservedAuthorization::new(reference, evaluated.decision),
        ))
    }

    async fn authorize_signal_call(
        &self,
        socket: std::path::PathBuf,
        authorization: SignalCallAuthorization,
        reference: AuthorizedObjectReference,
    ) -> Result<AuthorizationObservation, CriomeGateError> {
        let send_result = tokio::task::spawn_blocking(move || {
            CriomeClient::new(socket).send(CriomeRequest::AuthorizeSignalCall(authorization))
        })
        .await
        .map_err(|source| CriomeGateError::AuthorizationTask {
            message: source.to_string(),
        })?;
        let reply = match send_result {
            Ok(reply) => reply,
            Err(_) => return Ok(AuthorizationObservation::Unreachable),
        };
        let CriomeReply::AuthorizationGranted(grant) = reply else {
            return Err(CriomeGateError::UnexpectedReply {
                reply: format!("{reply:?}"),
            });
        };
        if grant.authorized_object_digest != reference.digest {
            return Err(CriomeGateError::UnexpectedReply {
                reply: format!("{grant:?}"),
            });
        }
        Ok(AuthorizationObservation::Observed(
            ObservedAuthorization::new(reference, EvaluationDecision::Authorized),
        ))
    }
}

impl std::fmt::Debug for CriomeGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CriomeGate")
            .field("armed", &self.is_armed())
            .field("socket", &self.socket_path())
            .finish()
    }
}

/// The gate's typed failure modes. An UNCONFIGURED, DENIED, or UNREACHABLE
/// criome is NOT an error — those are [`GateDecision`] outcomes the daemon
/// handles by holding the head back. An error here is a real fault in the
/// gate's own machinery: the blocking task panicked/cancelled, or criome
/// answered with an off-contract reply variant.
#[derive(Debug, Error)]
pub enum CriomeGateError {
    #[error("criome authorization task failed: {message}")]
    AuthorizationTask { message: String },

    #[error("criome answered with an unexpected reply: {reply}")]
    UnexpectedReply { reply: String },
}
