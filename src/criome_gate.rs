//! The criome head-authorization seam (Spirit `xhwa`) — DORMANT.
//!
//! A spirit working commit lands LOCALLY first (the durable SEMA log write in
//! `Engine::handle_async`). Before that committed head fans out to the mirror,
//! the daemon asks this gate to authorize the content-addressed head `D`, and
//! ships only on an explicit [`GateDecision::Authorized`].
//!
//! The earlier 1-of-1 LOCAL criome authorization path — a co-resident local
//! criome daemon answering over the per-user Unix socket, armed with a
//! deploy-config attestor or the socket-only bootstrap request — is DELETED,
//! not kept as a mode. The coming criome-cluster authorization flow owns this
//! seam and will wire a real cluster authorizer behind
//! [`CriomeGate::authorize_head`]. Until then the gate holds no authorizer and
//! every authorization request answers [`GateDecision::Unconfigured`]:
//! fail-closed, a missing authorizer is a missing authorization gate, never
//! permission to ship.
//!
//! Whether the seam runs at all is the gate's [`CriomeAuthorization`] policy:
//! `Disabled` (the operative default) keeps the whole authorize-and-ship seam
//! dormant — heads advance freely and nothing propagates; `Enabled` refuses
//! head advances fail-closed until the cluster authorizer exists.

#[cfg(feature = "agent-guardian")]
use criome::transport::CriomeClient;
use sema_engine::EntryDigest;
use signal_criome::{
    AuthorizedObjectKind, AuthorizedObjectReference, ComponentKind, EvaluationDecision,
    ObjectDigest,
};

use crate::schema::signal::Input;
#[cfg(feature = "agent-guardian")]
use signal_criome::{
    AuthorizationObservation as SignalAuthorizationObservation, AuthorizationPending,
    AuthorizationRequestSlot, AuthorizationScope, AuthorizationStateRecord, AuthorizationStatus,
    ContractName, ContractOperationHead, CriomeReply, CriomeRequest, Identity, ReplayNonce,
    SignalCallAuthorization, SpiritAuthorizationContext, SpiritProcessKey,
};
use thiserror::Error;

#[cfg(feature = "agent-guardian")]
const PARKED_AUTHORIZATION_OBSERVATION_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(100);
#[cfg(feature = "agent-guardian")]
const PARKED_AUTHORIZATION_OBSERVATION_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(300);

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

/// The spirit-side criome authorization policy — a closed typed option, not a
/// flag. It decides whether spirit's heads are subject to criome authorization
/// at all.
///
/// [`Disabled`](CriomeAuthorization::Disabled) is the operative default until
/// criome-cluster authorization is ready: spirit runs fully local, heads
/// advance freely, and nothing propagates — the authorize-and-ship seam
/// ([`CriomeGate::authorize_head`], `MirrorShipper`) stays dormant.
///
/// [`Enabled`](CriomeAuthorization::Enabled) demands criome authorization for
/// every head advance. No cluster authorizer exists yet, so an enabled gate
/// refuses head-advancing working inputs fail-closed; the coming
/// criome-cluster authorization flow will carry the authorizer configuration
/// on this variant.
///
/// The owner-only meta plane (`Import`, `CollectRemovalCandidates`) stays
/// owner-trust and is not policed by this option.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CriomeAuthorization {
    /// Spirit is fully local: heads advance freely, nothing propagates.
    #[default]
    Disabled,
    /// Every head advance requires criome authorization. With no cluster
    /// authorizer available yet, head advances are refused, fail-closed.
    Enabled,
}

impl CriomeAuthorization {
    /// The contact point between the authorization policy and a working
    /// input: whether the input may enter the Signal -> Nexus -> SEMA
    /// pipeline.
    ///
    /// `Disabled` admits everything. `Enabled` refuses every input that would
    /// advance the versioned-log head (any SEMA log write, including `State`,
    /// whose classification lands as a `Record` write) because no cluster
    /// authorizer exists yet to authorize the advance — fail-closed. Reads,
    /// subscriptions, and runtime-only taps carry no head advance and stay
    /// admitted. `ApplyAuthorizedRecord` is admitted so the pipeline answers
    /// it with its contract-shaped fail-closed `ApplyRefusal` (no store
    /// write).
    pub fn admits(&self, input: &Input) -> bool {
        match self {
            CriomeAuthorization::Disabled => true,
            CriomeAuthorization::Enabled => match input {
                Input::State(_)
                | Input::Record(_)
                | Input::Propose(_)
                | Input::Clarify(_)
                | Input::ResolveClarification(_)
                | Input::Supersede(_)
                | Input::Retire(_)
                | Input::BumpImportance(_)
                | Input::ChangeRecord(_)
                | Input::RegisterReferent(_) => false,
                Input::Observe(_)
                | Input::PublicTextSearch(_)
                | Input::PublicRecords(_)
                | Input::PrivateRecords(_)
                | Input::Lookup(_)
                | Input::Count(_)
                | Input::LookupStash(_)
                | Input::Tap(_)
                | Input::Untap(_)
                | Input::SubscribeIntent(_)
                | Input::ApplyAuthorizedRecord(_)
                | Input::Version
                | Input::Marker => true,
            },
        }
    }
}

/// The decision the gate returns to the daemon's fan-out point. Only
/// [`GateDecision::Authorized`] releases the ship; every other decision holds
/// the head back — the local commit stands and the suffix waits for the next
/// authorized drain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateDecision {
    /// criome authorized head `D`. Carries the projected reference so the
    /// daemon emits the SAME reference it authorized.
    Authorized(AuthorizedObjectReference),
    /// No criome-cluster authorizer is configured. Do not ship: this is a
    /// missing authorization gate, not permission to fan out. The dormant seam
    /// answers this unconditionally until the cluster flow arrives.
    Unconfigured,
    /// criome reached a decision but did not authorize (rejected quorum/time/
    /// signature, or escalated to the psyche). Do not ship.
    Denied(EvaluationDecision),
    /// criome was not reachable. Do not ship; the local commit stands and the
    /// suffix waits for the next drain.
    Unreachable,
}

impl GateDecision {
    /// Whether this decision releases the fan-out.
    pub fn ships(&self) -> bool {
        matches!(self, GateDecision::Authorized(_))
    }
}

/// The criome head-authorization gate — the seam the coming criome-cluster
/// authorization flow will drive. It carries the spirit-side
/// [`CriomeAuthorization`] policy and holds NO authorizer today: the 1-of-1
/// LOCAL criome path (socket + attestor arming) is deleted, so an enabled
/// gate answers every authorization request [`GateDecision::Unconfigured`],
/// fail-closed.
#[derive(Debug, Default)]
pub struct CriomeGate {
    authorization: CriomeAuthorization,
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
    pub fn authorization(&self) -> CriomeAuthorization {
        self.authorization
    }

    /// Authorize a captured head `D` before fan-out. The seam is DORMANT: no
    /// criome-cluster authorizer is wired yet, so this answers
    /// [`GateDecision::Unconfigured`] unconditionally — the head is held back,
    /// fail-closed. The coming cluster-authorization flow replaces this body
    /// with a real cluster round-trip; the signature (capture in, decision out,
    /// machinery faults as [`CriomeGateError`]) is the stable seam contract.
    pub async fn authorize_head(
        &self,
        capture: &LocalHeadCapture,
    ) -> Result<GateDecision, CriomeGateError> {
        let _ = capture;
        Ok(GateDecision::Unconfigured)
    }
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

    pub async fn authorize(
        &self,
        context: SpiritAuthorizationContext,
        mode: signal_spirit::AuthorizationMode,
    ) -> Result<SpiritOperationAuthorization, CriomeGateError> {
        let Some(socket) = self.socket.clone() else {
            return Ok(SpiritOperationAuthorization::Allowed);
        };
        let request_digest = ObjectDigest::from_bytes(context.raw_payload.payload().as_bytes());
        let authorization = self.signal_call_authorization(context, request_digest.clone());
        let client_socket = socket.clone();
        let send_result = tokio::task::spawn_blocking(move || {
            CriomeClient::new(client_socket).send(CriomeRequest::AuthorizeSignalCall(authorization))
        })
        .await
        .map_err(|source| CriomeGateError::AuthorizationTask {
            message: source.to_string(),
        })?;
        let reply = match send_result {
            Ok(reply) => reply,
            Err(_) => {
                return Ok(SpiritOperationAuthorization::Blocked(
                    "criome operation authorization unreachable".to_owned(),
                ));
            }
        };
        if mode == signal_spirit::AuthorizationMode::Gating
            && let CriomeReply::AuthorizationPending(pending) = reply
        {
            return self
                .wait_for_pending_authorization(socket, pending, request_digest)
                .await;
        }
        self.authorization_from_reply(reply, request_digest, mode)
    }

    fn signal_call_authorization(
        &self,
        context: SpiritAuthorizationContext,
        request_digest: ObjectDigest,
    ) -> SignalCallAuthorization {
        SignalCallAuthorization::new(
            request_digest,
            ContractName::new("signal-spirit"),
            ContractOperationHead::new(context.operation_name.payload().clone()),
            AuthorizationScope::new("spirit-operation"),
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

    fn authorization_from_reply(
        &self,
        reply: CriomeReply,
        request_digest: ObjectDigest,
        mode: signal_spirit::AuthorizationMode,
    ) -> Result<SpiritOperationAuthorization, CriomeGateError> {
        match mode {
            signal_spirit::AuthorizationMode::Observing => {
                self.validate_observed_reply(reply, request_digest)?;
                Ok(SpiritOperationAuthorization::Allowed)
            }
            signal_spirit::AuthorizationMode::Gating => match reply {
                CriomeReply::AuthorizationGranted(grant) => {
                    if grant.authorized_object_digest == request_digest {
                        Ok(SpiritOperationAuthorization::Allowed)
                    } else {
                        Err(CriomeGateError::UnexpectedReply {
                            reply: format!("{grant:?}"),
                        })
                    }
                }
                CriomeReply::AuthorizationPending(_)
                | CriomeReply::AuthorizationDenied(_)
                | CriomeReply::AuthorizationExpired(_)
                | CriomeReply::AuthorizationUnavailable(_) => {
                    Ok(SpiritOperationAuthorization::Blocked(format!("{reply:?}")))
                }
                other => Err(CriomeGateError::UnexpectedReply {
                    reply: format!("{other:?}"),
                }),
            },
        }
    }

    async fn wait_for_pending_authorization(
        &self,
        socket: std::path::PathBuf,
        pending: AuthorizationPending,
        request_digest: ObjectDigest,
    ) -> Result<SpiritOperationAuthorization, CriomeGateError> {
        let deadline = std::time::Instant::now() + PARKED_AUTHORIZATION_OBSERVATION_TIMEOUT;
        loop {
            if std::time::Instant::now() >= deadline {
                return Ok(SpiritOperationAuthorization::Blocked(format!(
                    "criome operation authorization timed out waiting for parked request {}",
                    pending.request_slot.payload()
                )));
            }
            match self
                .observe_pending_authorization(
                    socket.clone(),
                    pending.request_slot.clone(),
                    request_digest.clone(),
                )
                .await?
            {
                PendingAuthorizationObservation::Waiting => {}
                PendingAuthorizationObservation::Allowed => {
                    return Ok(SpiritOperationAuthorization::Allowed);
                }
                PendingAuthorizationObservation::Blocked(reason) => {
                    return Ok(SpiritOperationAuthorization::Blocked(reason));
                }
            }
        }
    }

    async fn observe_pending_authorization(
        &self,
        socket: std::path::PathBuf,
        request_slot: AuthorizationRequestSlot,
        request_digest: ObjectDigest,
    ) -> Result<PendingAuthorizationObservation, CriomeGateError> {
        let send_result = tokio::task::spawn_blocking(move || {
            std::thread::sleep(PARKED_AUTHORIZATION_OBSERVATION_INTERVAL);
            CriomeClient::new(socket).send(CriomeRequest::ObserveAuthorization(
                SignalAuthorizationObservation::new(request_slot),
            ))
        })
        .await
        .map_err(|source| CriomeGateError::AuthorizationTask {
            message: source.to_string(),
        })?;
        let reply = match send_result {
            Ok(reply) => reply,
            Err(_) => {
                return Ok(PendingAuthorizationObservation::Blocked(
                    "criome operation authorization unreachable while waiting".to_owned(),
                ));
            }
        };
        let CriomeReply::AuthorizationObservationSnapshot(snapshot) = reply else {
            return Err(CriomeGateError::UnexpectedReply {
                reply: format!("{reply:?}"),
            });
        };
        let Some(state) = snapshot.states().first() else {
            return Ok(PendingAuthorizationObservation::Waiting);
        };
        self.pending_authorization_from_state(state, request_digest)
    }

    fn pending_authorization_from_state(
        &self,
        state: &AuthorizationStateRecord,
        request_digest: ObjectDigest,
    ) -> Result<PendingAuthorizationObservation, CriomeGateError> {
        if state.request_digest != request_digest {
            return Err(CriomeGateError::UnexpectedReply {
                reply: format!("{state:?}"),
            });
        }
        match state.status {
            AuthorizationStatus::Granted => match state.grant() {
                Some(grant) if grant.authorized_object_digest == request_digest => {
                    Ok(PendingAuthorizationObservation::Allowed)
                }
                _ => Err(CriomeGateError::UnexpectedReply {
                    reply: format!("{state:?}"),
                }),
            },
            AuthorizationStatus::Denied => Ok(PendingAuthorizationObservation::Blocked(format!(
                "criome operation authorization denied: {:?}",
                state.denial()
            ))),
            AuthorizationStatus::Expired => Ok(PendingAuthorizationObservation::Blocked(
                "criome operation authorization expired".to_owned(),
            )),
            AuthorizationStatus::Unavailable => Ok(PendingAuthorizationObservation::Blocked(
                "criome operation authorization unavailable".to_owned(),
            )),
            AuthorizationStatus::Pending
            | AuthorizationStatus::Signing
            | AuthorizationStatus::Parked => Ok(PendingAuthorizationObservation::Waiting),
        }
    }

    fn validate_observed_reply(
        &self,
        reply: CriomeReply,
        request_digest: ObjectDigest,
    ) -> Result<(), CriomeGateError> {
        match reply {
            CriomeReply::AuthorizationGranted(grant) => {
                if grant.authorized_object_digest == request_digest {
                    Ok(())
                } else {
                    Err(CriomeGateError::UnexpectedReply {
                        reply: format!("{grant:?}"),
                    })
                }
            }
            CriomeReply::AuthorizationPending(_)
            | CriomeReply::AuthorizationDenied(_)
            | CriomeReply::AuthorizationExpired(_)
            | CriomeReply::AuthorizationUnavailable(_) => Ok(()),
            other => Err(CriomeGateError::UnexpectedReply {
                reply: format!("{other:?}"),
            }),
        }
    }
}

#[cfg(feature = "agent-guardian")]
enum PendingAuthorizationObservation {
    Waiting,
    Allowed,
    Blocked(String),
}

#[cfg(feature = "agent-guardian")]
impl Default for SpiritOperationAuthorizer {
    fn default() -> Self {
        Self::new()
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
