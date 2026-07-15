use std::{collections::HashMap, convert::Infallible, sync::Mutex as StdMutex};

#[cfg(feature = "mirror-shipper")]
use crate::shipper::{MirrorShipper, MirrorShipperError};
use crate::{
    nexus::Nexus,
    schema::{
        meta_signal::{
            CollectRemovalCandidatesRequest, ConfigureReceipt, ConfigureRejection,
            ConfigureRejectionReason, ConfigureRequest, HeadDigestHex, HeadObjectHex,
            ImportReceipt, ImportRequest, Output as MetaOutput, RemovalCandidatesCollectedReceipt,
            SelectedHeadDigest, SelectedHeadObject, VersionedLogHead, VersionedLogHeadObject,
        },
        nexus::{self as nexus_schema, NexusAction, NexusEngine, NexusWork},
        sema::ErrorReport,
        signal::{
            self as signal_schema, DatabaseMarker, ErrorMessage, Input, Integer, IntentEvent,
            Output, RecordCount, SemaReceipt, SupersessionReceipt, ValidationError,
        },
    },
    store::{Store, StoreError},
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, TraceEvent, TraceLog};

const ORIGIN_ROUTE_BASE: Integer = 1_000_000;

/// The composed failure modes of the gated head fan-out
/// ([`Engine::drain_propagation_once`]): a head-capture / store read failure,
/// a gate-machinery failure (the blocking criome task, an off-contract reply,
/// or a session binding violation), or a mirror-shipper transport failure on
/// the authorized ship. A REFUSED or UNREACHABLE criome is NOT an error — it
/// is a [`crate::criome_gate::GateDecision`] the drain handles by holding the
/// head back.
#[cfg(feature = "mirror-shipper")]
#[derive(Debug, thiserror::Error)]
pub enum GateAndShipError {
    #[error("head capture / store read failed: {0}")]
    Store(#[from] StoreError),

    #[error("criome gate failed: {0}")]
    Gate(#[from] crate::criome_gate::CriomeGateError),

    #[error("authorized mirror ship failed: {0}")]
    Ship(#[from] MirrorShipperError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OriginRoute(Integer);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageIdentifier(Integer);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageSent {
    pub identifier: MessageIdentifier,
    origin_route: OriginRoute,
    pub short_header: Integer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageProcessed<Root> {
    identifier: MessageIdentifier,
    origin_route: OriginRoute,
    root: Root,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentMail {
    pub mail_identifier: MailIdentifier,
    pub origin_route: OriginRoute,
    pub short_header: ShortHeader,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessedMail {
    pub mail_identifier: MailIdentifier,
    pub origin_route: OriginRoute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MailIdentifier(Integer);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortHeader(Integer);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MailLedgerEvent {
    Sent(SentMail),
    Processed(ProcessedMail),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalResponse<Root> {
    origin_route: OriginRoute,
    root: Root,
}

#[cfg_attr(feature = "nota-text", derive(nota::NotaDecode, nota::NotaEncode))]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalObjectName {
    Started,
    Stopped,
    Admitted,
    Rejected,
    Triaged,
    Replied,
}

pub trait MessageSentHook {
    type Error;

    fn message_sent(&mut self, event: MessageSent) -> Result<(), Self::Error>;
}

pub trait MessageProcessedHook<Root> {
    type Error;

    fn message_processed(&mut self, event: MessageProcessed<Root>) -> Result<(), Self::Error>;
}

impl OriginRoute {
    pub fn new(payload: Integer) -> Self {
        Self(payload)
    }

    pub fn payload(&self) -> Integer {
        self.0
    }
}

impl MessageIdentifier {
    pub fn new(payload: Integer) -> Self {
        Self(payload)
    }

    pub fn payload(&self) -> Integer {
        self.0
    }

    pub fn as_integer(&self) -> Integer {
        self.payload()
    }
}

impl MailIdentifier {
    pub fn new(payload: Integer) -> Self {
        Self(payload)
    }

    pub fn payload(&self) -> Integer {
        self.0
    }
}

impl ShortHeader {
    pub fn new(payload: Integer) -> Self {
        Self(payload)
    }

    pub fn payload(&self) -> Integer {
        self.0
    }
}

impl MessageSent {
    pub fn new(
        identifier: MessageIdentifier,
        origin_route: OriginRoute,
        short_header: Integer,
    ) -> Self {
        Self {
            identifier,
            origin_route,
            short_header,
        }
    }

    pub fn origin_route(&self) -> OriginRoute {
        self.origin_route
    }

    pub fn push_to<Hook>(self, hook: &mut Hook) -> Result<(), Hook::Error>
    where
        Hook: MessageSentHook,
    {
        hook.message_sent(self)
    }

    pub fn into_mail_ledger_event(self) -> MailLedgerEvent {
        MailLedgerEvent::sent(SentMail {
            mail_identifier: MailIdentifier::new(self.identifier.as_integer()),
            origin_route: self.origin_route(),
            short_header: ShortHeader::new(self.short_header),
        })
    }
}

impl<Root> MessageProcessed<Root> {
    pub fn new(identifier: MessageIdentifier, origin_route: OriginRoute, root: Root) -> Self {
        Self {
            identifier,
            origin_route,
            root,
        }
    }

    pub fn identifier(&self) -> MessageIdentifier {
        self.identifier
    }

    pub fn origin_route(&self) -> OriginRoute {
        self.origin_route
    }

    pub fn root(&self) -> &Root {
        &self.root
    }

    pub fn push_to<Hook>(self, hook: &mut Hook) -> Result<(), Hook::Error>
    where
        Hook: MessageProcessedHook<Root>,
    {
        hook.message_processed(self)
    }
}

impl MessageProcessed<Output> {
    pub fn processed_mail_event(&self) -> MailLedgerEvent {
        MailLedgerEvent::processed(ProcessedMail {
            mail_identifier: MailIdentifier::new(self.identifier().as_integer()),
            origin_route: self.origin_route(),
        })
    }
}

impl MailLedgerEvent {
    pub fn sent(payload: SentMail) -> Self {
        Self::Sent(payload)
    }

    pub fn processed(payload: ProcessedMail) -> Self {
        Self::Processed(payload)
    }

    pub fn is_sent(&self) -> bool {
        matches!(self, Self::Sent(_))
    }

    pub fn is_processed(&self) -> bool {
        matches!(self, Self::Processed(_))
    }
}

impl<Root> SignalResponse<Root> {
    pub fn new(origin_route: OriginRoute, root: Root) -> Self {
        Self { origin_route, root }
    }

    pub fn origin_route(&self) -> OriginRoute {
        self.origin_route
    }

    pub fn root(&self) -> &Root {
        &self.root
    }

    pub fn into_root(self) -> Root {
        self.root
    }
}

impl SignalObjectName {
    pub fn name(self) -> &'static str {
        match self {
            Self::Started => "SignalStarted",
            Self::Stopped => "SignalStopped",
            Self::Admitted => "SignalAdmitted",
            Self::Rejected => "SignalRejected",
            Self::Triaged => "SignalTriaged",
            Self::Replied => "SignalReplied",
        }
    }
}

/// The daemon runtime: a thin composer of the three execution centers.
///
/// `Engine` owns the Signal admission gate and the Nexus mail keeper.
/// Nexus owns the durable SEMA store and the mail ledger. `Engine::handle`
/// runs the record-970 flow as a composition — it does NOT call the store
/// directly; the SEMA invocation lives inside Nexus, which holds the mail
/// in a being-processed state across it.
///
/// The engine is owned by the schema-emitted `EngineActor` kameo actor: the
/// actor mailbox serialises every working request, so `Engine` holds its
/// Nexus as a plain field and mutates it through `&mut self` — no internal
/// lock guards the single-flight working path. The Signal admission gate keeps
/// its small `StdMutex` identity counters because it is borrowed shared
/// (`&self`) by the `SignalEngine::triage` / `reply` plane.
#[derive(Debug)]
pub struct Engine {
    signal_admission: SignalAdmission,
    nexus: Nexus,
    authorization_mode: signal_spirit::AuthorizationMode,
    // The OFF-by-default mirror gate. Present only under the `mirror-shipper`
    // feature (which the binary-only daemon build excludes to keep nota
    // out of its dependency tree). Even when built in, it is unarmed until an
    // owner `Configure` carries a `MirrorTarget::Address`, so a daemon that
    // never receives a mirror target behaves identically to one built before
    // mirroring existed.
    #[cfg(feature = "mirror-shipper")]
    mirror_shipper: MirrorShipper,
    // The criome acceptance gate (Spirit `xhwa`, the everywhere-gate), LIVE.
    // Its own `criome-gate` feature — acceptance gating must never be
    // compiled out together with shipping. It carries the spirit-side
    // `CriomeAuthorization` policy: `Disabled` (the operative default) keeps
    // the whole seam dormant — heads advance freely, nothing propagates;
    // `Enabled` STAGES every head-advancing working operation and appends it
    // only on the cluster grant; every other terminal verdict refuses the
    // operation to the caller, fail-closed, nothing recorded anywhere.
    #[cfg(feature = "criome-gate")]
    criome_gate: crate::criome_gate::CriomeGate,
    // The first-in first-out advance gate (§3.5.3): staged working turns
    // serialize among THEMSELVES across stage, authorize, and materialize —
    // one outstanding staged group, one outstanding round — while reads flow
    // through the actor mailbox between the fast turns. Shared with the
    // emitted daemon spine (`SpiritDaemon::shared_advance_gate`) and with the
    // ship drain's residue reconcile pass.
    #[cfg(feature = "criome-gate")]
    advance_gate: std::sync::Arc<tokio::sync::Mutex<()>>,
    // The ship drain (§3.6): present exactly when the gate is Enabled and the
    // shipper is built in. Each materialization sends it "head advanced" mail
    // (`notify_head_advanced`) — distribution of already-accepted state,
    // never a second acceptance judgment; tests drive it directly through
    // `drain_propagation_once`.
    #[cfg(feature = "mirror-shipper")]
    propagation: Option<std::sync::Arc<crate::propagation::PropagationDrain>>,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
}

/// The stage turn's verdict (§3.5.1): the input either completed outright —
/// a read, a refusal, a Disabled-mode direct write, or an accepting operation
/// that appended nothing — or its operation group is durably parked awaiting
/// the cluster round.
#[cfg(feature = "criome-gate")]
#[derive(Debug)]
pub enum StagedIntake {
    Completed(Output),
    Parked(crate::criome_gate::StagedHeadAdvance),
}

/// The closed Output → staged-group-fate contact point (§3.5.1): an ACCEPTING
/// reply is the held acceptance of a head-advancing operation, so its staged
/// group opens a round and materializes on the grant; every other reply
/// refuses or answers without appending, so its staged buffer is abandoned —
/// nothing recorded anywhere. The match is exhaustive on purpose: a new
/// Output variant must choose its fate here before spirit compiles.
#[cfg(feature = "criome-gate")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplyFate {
    Accepting,
    NonAccepting,
}

#[cfg(feature = "criome-gate")]
impl ReplyFate {
    fn of(output: &Output) -> Self {
        match output {
            Output::RecordAccepted(_)
            | Output::Proposed(_)
            | Output::Clarified(_)
            | Output::ClarificationResolved(_)
            | Output::Superseded(_)
            | Output::Retired(_)
            | Output::CertaintyChanged(_)
            | Output::ImportanceBumped(_)
            | Output::RecordChanged(_)
            | Output::ReferentRegistered(_) => Self::Accepting,
            // `RecordApplied` is the §4 acceptance-by-verification reply: the
            // carried authorization already gated that state cluster-wide, so
            // an apply never opens an intake round (and today's nexus answers
            // the apply ingress fail-closed, never producing it).
            Output::Error(_)
            | Output::Rejected(_)
            | Output::GuardianRejected(_)
            | Output::ReferentGuardianRejected(_)
            | Output::ApplyRefused(_)
            | Output::AdvanceRefused(_)
            | Output::RecordApplied(_)
            | Output::RecordsObserved(_)
            | Output::RecordsStashed(_)
            | Output::RecordFound(_)
            | Output::RecordsCounted(_)
            | Output::SubscriptionStarted(_)
            | Output::ObservationTapped(_)
            | Output::ObservationUntapped(_)
            | Output::Event(_)
            | Output::VersionReported(_)
            | Output::MarkerReported(_) => Self::NonAccepting,
        }
    }
}

/// The Signal admission gate: the request-admission plane that mints the
/// origin route, issues a message identifier, and validates a wire `Input`
/// before any deeper layer sees it, plus the `SignalEngine` triage / reply
/// translation between the Signal and Nexus planes.
///
/// This is NOT a kameo actor — the kameo actor that owns the engine is the
/// schema-emitted `EngineActor`. The admission gate is a data-bearing plane
/// the engine composes; its identity counters live behind small `StdMutex`es
/// because the `SignalEngine` plane borrows it shared.
#[derive(Debug, Default)]
pub struct SignalAdmission {
    next_message_identifier: StdMutex<Integer>,
    next_origin_route: StdMutex<Integer>,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
}

#[derive(Debug)]
pub struct SignalAccepted {
    input: Input,
    origin_route: OriginRoute,
    sent: MessageSent,
}

#[derive(Debug)]
pub struct SignalRejected {
    origin_route: OriginRoute,
    validation_error: ValidationError,
}

/// The current in-flight mail state owned by Nexus.
///
/// A sent mail marker is retained only until its matching terminal processed
/// marker arrives. Processed markers are lifecycle transitions, not history, so
/// completed or duplicate terminal events leave no daemon-lifetime retention.
#[derive(Debug, Default)]
pub struct MailLedger {
    state: StdMutex<MailLedgerState>,
}

/// The bounded mail lifecycle state: current sent mail plus aggregate metrics.
#[derive(Debug, Default)]
struct MailLedgerState {
    in_flight: HashMap<Integer, SentMail>,
    sent_count: usize,
    processed_count: usize,
}

#[derive(Debug)]
pub struct MailLedgerHook<'a> {
    ledger: &'a MailLedger,
}

impl Engine {
    /// Build the runtime over a durable SEMA store opened at `.sema` path.
    pub fn new(store: Store) -> Self {
        #[cfg(feature = "testing-trace")]
        {
            Self::new_with_trace(store, TraceLog::default())
        }
        #[cfg(not(feature = "testing-trace"))]
        {
            Self {
                signal_admission: SignalAdmission::default(),
                nexus: Nexus::new(store),
                authorization_mode: signal_spirit::AuthorizationMode::Gating,
                #[cfg(feature = "mirror-shipper")]
                mirror_shipper: MirrorShipper::new(),
                #[cfg(feature = "criome-gate")]
                criome_gate: crate::criome_gate::CriomeGate::new(),
                #[cfg(feature = "criome-gate")]
                advance_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
                #[cfg(feature = "mirror-shipper")]
                propagation: None,
            }
        }
    }

    #[cfg(feature = "agent-guardian")]
    pub fn set_guardian(&mut self, guardian: crate::guardian::AgentGuardian) {
        self.nexus.set_guardian(guardian);
    }

    #[cfg(feature = "agent-guardian")]
    pub fn require_guardian(&mut self) {
        self.nexus.require_guardian();
    }

    #[cfg(feature = "testing-trace")]
    pub fn new_with_trace(store: Store, trace_log: TraceLog) -> Self {
        Self {
            signal_admission: SignalAdmission::with_trace(trace_log.clone()),
            nexus: Nexus::new_with_trace(store, trace_log.clone()),
            authorization_mode: signal_spirit::AuthorizationMode::Gating,
            #[cfg(feature = "mirror-shipper")]
            mirror_shipper: MirrorShipper::new(),
            #[cfg(feature = "criome-gate")]
            criome_gate: crate::criome_gate::CriomeGate::new(),
            #[cfg(feature = "criome-gate")]
            advance_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(feature = "mirror-shipper")]
            propagation: None,
            trace_log,
        }
    }

    #[cfg(feature = "testing-trace")]
    pub fn trace_events(&self) -> Vec<TraceEvent> {
        self.trace_log.events()
    }

    pub fn start(&mut self) -> Result<(), nexus_schema::EngineStartFailure> {
        NexusEngine::on_start(&mut self.nexus)?;
        self.signal_admission.start();
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), nexus_schema::EngineStopFailure> {
        self.signal_admission.stop();
        NexusEngine::on_stop(&mut self.nexus)?;
        Ok(())
    }

    pub fn set_authorization_mode(&mut self, authorization_mode: signal_spirit::AuthorizationMode) {
        self.authorization_mode = authorization_mode;
        #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
        self.nexus
            .set_operation_authorization_mode(authorization_mode);
    }

    pub fn authorization_mode(&self) -> signal_spirit::AuthorizationMode {
        self.authorization_mode
    }

    /// Run one request through Signal admission, the NexusEngine
    /// composition, and the durable SEMA store.
    ///
    /// Signal admits the input (mints the origin route, issues an
    /// identifier, and validates) before any deeper layer sees it. The
    /// sent hook fires at the Signal→Nexus handoff; the processed hook
    /// fires after the NexusEngine returns its reply.
    pub fn handle(&mut self, input: Input) -> SignalResponse<Output> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("spirit sync handle runtime")
            .block_on(self.handle_async(input))
    }

    pub async fn handle_async(&mut self, input: Input) -> SignalResponse<Output> {
        let accepted = match self.signal_admission.admit(input) {
            Ok(accepted) => accepted,
            Err(rejected) => {
                let output = rejected.into_signal_output();
                #[cfg(feature = "testing-trace")]
                self.signal_admission.trace_signal_rejected();
                #[cfg(feature = "testing-trace")]
                self.signal_admission.trace_signal_replied();
                return output;
            }
        };
        accepted
            .process_with(&self.signal_admission, &mut self.nexus)
            .await
    }

    pub fn record_count(&self) -> usize {
        self.nexus.store().len()
    }

    /// The durable SEMA store this engine composes — its query surface,
    /// versioned log, and shared engine handle.
    pub fn store(&self) -> &Store {
        self.nexus.store()
    }

    /// The live recovery stashes awaiting their one `LookupStash` consumer.
    pub fn stash_table(&self) -> &crate::nexus::StashTable {
        self.nexus.stash_table()
    }

    /// The live observer registry, retaining only operations an active tap can
    /// still consume.
    pub fn observer_tap_table(&self) -> &crate::nexus::ObserverTapTable {
        self.nexus.observer_tap_table()
    }

    /// The current versioned-log head `EntryDigest` after the latest local
    /// commit. Under the everywhere-gate this only ever names ACCEPTED state:
    /// with the gate Enabled, a head-advancing operation materializes only
    /// after the cluster grant, so the local head never runs ahead of
    /// acceptance. `None` when nothing has been committed to the versioned
    /// log yet.
    #[cfg(feature = "criome-gate")]
    pub fn versioned_log_head(&self) -> Result<Option<sema_engine::EntryDigest>, StoreError> {
        self.nexus.store().versioned_log_head()
    }

    #[cfg(feature = "agent-guardian")]
    pub fn guardian_decision_count(&self) -> usize {
        self.nexus.store().guardian_decision_count().unwrap_or(0)
    }

    /// Prompt prose belongs to the external Spirit judge adapter. The daemon
    /// keeps this legacy diagnostic method only as a compatibility affordance.
    #[cfg(feature = "agent-guardian")]
    pub fn guardian_intent_system_prompt(&self) -> Option<String> {
        None
    }

    pub fn sent_message_count(&self) -> usize {
        self.nexus.mail_ledger().sent_message_count()
    }

    pub fn processed_message_count(&self) -> usize {
        self.nexus.mail_ledger().processed_message_count()
    }

    pub fn in_flight_mail_count(&self) -> usize {
        self.nexus.mail_ledger().in_flight_count()
    }

    pub fn mail_ledger(&self) -> Vec<MailLedgerEvent> {
        self.nexus.mail_ledger().events()
    }

    pub fn database_marker(&self) -> DatabaseMarker {
        self.nexus.database_marker()
    }

    /// Apply an owner-only meta `Configure` request: store WHERE the SEPARATE
    /// archive database lives, and reply with the now-active target plus the
    /// live database marker.
    ///
    /// This is the owner-config meta-socket effect. It records the archive
    /// target the peer-callable `CollectRemovalCandidates` will write to; it
    /// does NOT open, move, or touch the live database, and it never re-enters
    /// the Signal -> Nexus -> SEMA working pipeline (there is no SEMA log
    /// write). It takes `&mut self`, so the schema-emitted `EngineActor`
    /// mailbox serialises a reconfigure against every working write — a
    /// reconfigure and a working write can never run concurrently, without any
    /// component-internal lock. Storing the target is infallible — the archive
    /// database is opened lazily later, not here — so this always replies
    /// `Configured`. The `ConfigureRejection` / `ArchiveTargetUnwritable` arm of
    /// the contract is reserved for a future eager-validation policy.
    pub fn configure(&mut self, request: ConfigureRequest) -> MetaOutput {
        // `ConfigureRequest` is now a named-field struct carrying both the
        // archive target and an optional mirror target. The field plumbing is
        // unconditional so the DEFAULT build (where the gated shipper module is
        // absent) still compiles and echoes the mirror target back in the
        // receipt; only the actual shipper-arming below is feature-gated.
        let ConfigureRequest {
            archive_database_target,
            selected_mirror_target,
            selected_criome_gate_target,
            selected_guardian_prompt_target,
        } = request;
        let mirror_target = selected_mirror_target.into_payload();
        let criome_gate_target = selected_criome_gate_target.into_payload();
        let guardian_prompt_target = selected_guardian_prompt_target.into_payload();
        self.nexus
            .set_archive_target(archive_database_target.clone());
        // GuardianPromptTarget remains in the owner-only meta contract for
        // compatibility, but prompt text now belongs to the external Spirit
        // judge adapter/config. The daemon echoes the selected target and does
        // not compile, render, or apply prompt prose.
        // Arm (or disarm) the mirror shipper against the live engine handle.
        // A bad mirror address is an owner-config error, not a SEMA fault, so
        // it rejects the Configure rather than silently leaving mirroring off.
        // Present only when the gated shipper is built in; without it the
        // mirror target is echoed in the receipt but no shipper exists.
        #[cfg(feature = "mirror-shipper")]
        if let Err(error) = self
            .mirror_shipper
            .configure(mirror_target.as_ref(), self.nexus.store().engine_handle())
        {
            let _ = error;
            return MetaOutput::rejected(ConfigureRejection {
                configure_rejection_reason: ConfigureRejectionReason::InternalError,
                database_marker: self.nexus.database_marker(),
            });
        }
        // The criome gate target is the owner switch for the spirit-side
        // criome authorization policy: a socket target arms the CLUSTER
        // authorizer (`CriomeAuthorization::Enabled` — an enabled gate always
        // has a socket) and, under the guardian, the operation-level
        // authorizer at the same socket; `Default` or an absent target
        // disarms both (`Disabled`, spirit fully local).
        #[cfg(feature = "criome-gate")]
        {
            match &criome_gate_target {
                Some(crate::schema::meta_signal::CriomeGateTarget::Socket(socket_path)) => {
                    let socket = socket_path.payload().payload().clone();
                    self.criome_gate.set_authorization(
                        crate::criome_gate::CriomeAuthorization::Enabled(
                            crate::criome_gate::ClusterAuthorizer::new(socket.clone()),
                        ),
                    );
                    #[cfg(feature = "agent-guardian")]
                    self.nexus.configure_operation_authorizer(socket);
                }
                Some(crate::schema::meta_signal::CriomeGateTarget::Default) | None => {
                    self.criome_gate
                        .set_authorization(crate::criome_gate::CriomeAuthorization::Disabled);
                    #[cfg(feature = "agent-guardian")]
                    self.nexus.clear_operation_authorizer();
                }
            }
            #[cfg(feature = "mirror-shipper")]
            self.rebuild_propagation();
        }
        MetaOutput::configured(ConfigureReceipt::new(
            archive_database_target,
            mirror_target,
            criome_gate_target,
            guardian_prompt_target,
            self.nexus.database_marker(),
        ))
    }

    /// Whether the OFF-by-default mirror shipper is armed (an owner configured
    /// a `MirrorTarget::Address`).
    #[cfg(feature = "mirror-shipper")]
    pub fn mirror_shipping_armed(&self) -> bool {
        self.mirror_shipper.is_armed()
    }

    /// Drain the engine's unshipped versioned-log outbox to the configured
    /// mirror. A no-op when mirroring is off, so the daemon's post-commit hook
    /// calls it unconditionally. Shipping is best-effort relative to LOCAL
    /// durability: the working write already committed locally before this
    /// runs, so a mirror that is unreachable leaves the local log intact and
    /// the suffix waiting in the outbox for the next drain.
    #[cfg(feature = "mirror-shipper")]
    pub async fn ship_unshipped_to_mirror(
        &self,
    ) -> Result<Option<mirror::ShipOutcome>, MirrorShipperError> {
        self.mirror_shipper.ship_unshipped().await
    }

    /// Set the spirit-side [`crate::criome_gate::CriomeAuthorization`] policy.
    /// `Disabled` (the operative default) keeps spirit fully local; `Enabled`
    /// demands the cluster grant BEFORE any head-advancing working operation
    /// is accepted (§3.5) — refusal means nothing is recorded anywhere.
    #[cfg(feature = "criome-gate")]
    pub fn set_criome_authorization(
        &mut self,
        authorization: crate::criome_gate::CriomeAuthorization,
    ) {
        self.criome_gate.set_authorization(authorization);
        #[cfg(feature = "mirror-shipper")]
        self.rebuild_propagation();
    }

    /// The spirit-side criome authorization policy.
    #[cfg(feature = "criome-gate")]
    pub fn criome_authorization(&self) -> &crate::criome_gate::CriomeAuthorization {
        self.criome_gate.authorization()
    }

    /// The first-in first-out advance gate the emitted daemon spine and the
    /// ship drain share with the intake path (§3.5.3).
    #[cfg(feature = "criome-gate")]
    pub fn advance_gate(&self) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        std::sync::Arc::clone(&self.advance_gate)
    }

    /// Rebuild the ship drain from the current gate policy and mirror
    /// address: present exactly when the gate is Enabled, carrying its own
    /// shipper over the shared store engine (armed only when a mirror target
    /// is configured) and the shared advance gate.
    #[cfg(feature = "mirror-shipper")]
    fn rebuild_propagation(&mut self) {
        self.propagation = match self.criome_gate.authorization() {
            crate::criome_gate::CriomeAuthorization::Disabled => None,
            crate::criome_gate::CriomeAuthorization::Enabled(authorizer) => {
                let shipper = match self.mirror_shipper.address() {
                    Some(address) => {
                        MirrorShipper::armed(self.nexus.store().engine_handle(), address)
                    }
                    None => MirrorShipper::new(),
                };
                Some(std::sync::Arc::new(
                    crate::propagation::PropagationDrain::new(
                        authorizer.clone(),
                        self.nexus.store().engine_handle(),
                        shipper,
                        std::sync::Arc::clone(&self.advance_gate),
                    ),
                ))
            }
        };
    }

    /// THE SHIP DRAIN, driven directly (tests and deterministic callers).
    /// Capture the head of the unshipped suffix, fetch its grant (the
    /// standing re-grant in the steady state; ONE genuinely new batch round
    /// for disabled-era residue), and on Authorized ship the suffix up to
    /// exactly that head (§3.6). Never a second acceptance judgment.
    ///
    /// `Disabled` (the operative default) keeps the whole seam dormant: no
    /// head capture, no ask, no ship — it returns `None`, spirit fully
    /// local. An empty outbox (nothing to ship) is also a `None` no-op.
    #[cfg(feature = "mirror-shipper")]
    pub async fn drain_propagation_once(
        &self,
    ) -> Result<Option<crate::criome_gate::GateDecision>, GateAndShipError> {
        match &self.propagation {
            None => Ok(None),
            Some(drain) => drain.drain_once().await,
        }
    }

    /// THE SHIP TRIGGER: send the ship drain one "head advanced" mail and
    /// return immediately. Under the everywhere-gate this runs after a
    /// staged group MATERIALIZES (acceptance already happened at the grant)
    /// and once when enabling the gate owes a residue reconcile — it is
    /// distribution only, never part of acceptance. A dormant seam
    /// (Disabled) has no drain and pays nothing. Must be called from within
    /// a tokio runtime.
    #[cfg(feature = "mirror-shipper")]
    pub fn notify_head_advanced(&self) {
        if let Some(drain) = &self.propagation {
            let drain = std::sync::Arc::clone(drain);
            tokio::spawn(drain.drain_serialized());
        }
    }

    /// Publish the store's latest local checkpoint to the configured mirror,
    /// the portable restore body a fresh store fetches alongside the shipped
    /// log suffix. A no-op when mirroring is off.
    #[cfg(feature = "mirror-shipper")]
    pub async fn publish_checkpoint_to_mirror(&self) -> Result<bool, MirrorShipperError> {
        self.mirror_shipper.publish_checkpoint().await
    }

    /// THE INTAKE GATE'S STAGE PHASE (§3.5.1): run one working input in
    /// build mode — the ordinary Signal → Nexus → SEMA pipeline over the
    /// engaged staging session, reads against committed state plus the
    /// in-group overlay, nothing committed — and either complete the input
    /// outright or durably park its operation group for the cluster round.
    ///
    /// `Disabled` (the operative default) is today's direct path,
    /// byte-for-byte. Under `Enabled`: an ACCEPTING reply whose staged group
    /// is non-empty parks and awaits the grant (the reply is HELD — the
    /// caller hears nothing until the verdict); an accepting reply that
    /// appended nothing completes immediately (nothing to authorize); every
    /// other reply abandons the buffer — a guardian rejection or validation
    /// failure never opens a round and never leaves a trace. An occupied
    /// durable staging slot is an unresolved crash window (§3.8): head
    /// advances refuse `Unavailable` until an owner `Configure` re-enables
    /// the gate and recovery resolves the slot. Fail-closed at every fork.
    #[cfg(feature = "criome-gate")]
    pub async fn stage_working_input(&mut self, input: Input) -> StagedIntake {
        let authorizer = match self.criome_gate.authorization() {
            crate::criome_gate::CriomeAuthorization::Disabled => {
                return StagedIntake::Completed(self.handle_async(input).await.into_root());
            }
            crate::criome_gate::CriomeAuthorization::Enabled(authorizer) => authorizer.clone(),
        };
        let database = self.nexus.store().engine_handle();
        if let Err(occupied_or_engaged) = database.begin_staged_group() {
            eprintln!(
                "spirit intake refused a head advance (staging unavailable): \
                 {occupied_or_engaged}"
            );
            return StagedIntake::Completed(Output::advance_refused(
                signal_schema::AdvanceRefusal::new(
                    signal_schema::AdvanceRefusalReason::Unavailable,
                ),
            ));
        }
        let reply = self.handle_async(input).await.into_root();
        match ReplyFate::of(&reply) {
            ReplyFate::NonAccepting => {
                if let Err(fault) = database.abandon_staged_group() {
                    eprintln!("spirit staging abandon failed: {fault}");
                }
                StagedIntake::Completed(reply)
            }
            ReplyFate::Accepting => match database.park_staged_group() {
                Ok(None) => StagedIntake::Completed(reply),
                Ok(Some(receipt)) => {
                    StagedIntake::Parked(crate::criome_gate::StagedHeadAdvance::new(
                        authorizer,
                        receipt.prospective_head(),
                        reply,
                    ))
                }
                Err(fault) => {
                    // The group could not be durably parked: refuse the
                    // operation — nothing was committed, nothing survives.
                    eprintln!("spirit intake refused a head advance (park failed): {fault}");
                    if let Err(abandon_fault) = database.abandon_staged_group() {
                        eprintln!("spirit staging abandon failed: {abandon_fault}");
                    }
                    StagedIntake::Completed(Output::advance_refused(
                        signal_schema::AdvanceRefusal::new(
                            signal_schema::AdvanceRefusalReason::Unreachable,
                        ),
                    ))
                }
            },
        }
    }

    /// THE INTAKE GATE'S CONCLUDING PHASE (§3.5.1 step 3): after the
    /// connection task drained the authorization session, materialize the
    /// parked group on the grant — one atomic transaction, asserting the
    /// produced head equals the granted digest — and release the held
    /// accepted reply; on every other terminal verdict discard the parked
    /// group and refuse the operation with the typed reason. Acceptance
    /// happened at the grant; materialization is local bookkeeping of an
    /// already-accepted operation, so a materialization fault after the
    /// grant keeps the slot parked for recovery (§3.8 case 3) and reports
    /// `Unreachable` to the (now indeterminate) caller, loudly.
    #[cfg(feature = "criome-gate")]
    pub async fn conclude_staged_advance(
        &mut self,
        advance: crate::criome_gate::StagedHeadAdvance,
    ) -> Output {
        let database = self.nexus.store().engine_handle();
        let verdict = advance.verdict().cloned();
        let refusal_reason = match verdict {
            Some(crate::criome_gate::GateDecision::Authorized(_reference)) => {
                match database.materialize_staged_group(&self.nexus.store().family_directory()) {
                    Ok(_receipt) => {
                        #[cfg(feature = "mirror-shipper")]
                        self.notify_head_advanced();
                        return advance.into_held_reply();
                    }
                    Err(fault) => {
                        eprintln!(
                            "spirit could not materialize the GRANTED staged group {} — the \
                             slot stays parked for recovery: {fault}",
                            advance.prospective_head()
                        );
                        signal_schema::AdvanceRefusalReason::Unreachable
                    }
                }
            }
            Some(refused) => {
                let reason = refused
                    .refusal_reason()
                    .unwrap_or(signal_schema::AdvanceRefusalReason::Unreachable);
                if let Err(fault) = database.discard_staged_group() {
                    eprintln!("spirit staging discard failed: {fault}");
                }
                reason
            }
            None => {
                // Never resolved: fail-closed as unreachable machinery.
                if let Err(fault) = database.discard_staged_group() {
                    eprintln!("spirit staging discard failed: {fault}");
                }
                signal_schema::AdvanceRefusalReason::Unreachable
            }
        };
        Output::advance_refused(signal_schema::AdvanceRefusal::new(refusal_reason))
    }

    /// §3.8 crash recovery: resolve an occupied durable staging slot by
    /// re-asking the cluster with the parked prospective digest. From
    /// criome's view this is a first ask, an identical re-proposal
    /// re-opening the durable round, or the immediate re-grant of the
    /// standing committed head (the crash-after-grant case) — grant
    /// materializes, refusal discards, either way the slot resolves. Runs
    /// when an owner `Configure` enables the gate; until then an occupied
    /// slot refuses every head advance, fail-closed.
    #[cfg(feature = "criome-gate")]
    pub async fn resolve_parked_staged_group(&mut self) {
        let database = self.nexus.store().engine_handle();
        let parked = match database.staged_group() {
            Ok(Some(summary)) => summary,
            Ok(None) => return,
            Err(fault) => {
                eprintln!("spirit staging recovery could not read the slot: {fault}");
                return;
            }
        };
        let authorizer = match self.criome_gate.authorization() {
            crate::criome_gate::CriomeAuthorization::Disabled => return,
            crate::criome_gate::CriomeAuthorization::Enabled(authorizer) => authorizer.clone(),
        };
        let mut advance = crate::criome_gate::StagedHeadAdvance::new(
            authorizer,
            parked.prospective_head(),
            // The original caller's connection is gone (§3.5.4); the held
            // reply is unreachable bookkeeping and never delivered.
            Output::advance_refused(signal_schema::AdvanceRefusal::new(
                signal_schema::AdvanceRefusalReason::Unreachable,
            )),
        );
        advance.resolve().await;
        let digest = parked.prospective_head();
        let _conclusion = self.conclude_staged_advance(advance).await;
        match database.staged_group() {
            Ok(None) => eprintln!("spirit staging recovery resolved parked group {digest}"),
            Ok(Some(_still_parked)) => eprintln!(
                "spirit staging recovery left group {digest} parked (verdict did not resolve \
                 it); head advances stay refused"
            ),
            Err(fault) => eprintln!("spirit staging recovery could not re-read the slot: {fault}"),
        }
    }

    pub async fn configure_async(&mut self, request: ConfigureRequest) -> MetaOutput {
        let output = self.configure(request);
        #[cfg(feature = "criome-gate")]
        {
            // §3.8: an occupied durable staging slot resolves as soon as the
            // owner enables the gate — inside this actor turn, BEFORE any new
            // head advance can stage. Until resolved, staged intake refuses
            // every head advance, fail-closed.
            self.resolve_parked_staged_group().await;
            // §3.6: enabling the gate owes one reconcile mail — disabled-era
            // residue (an unshipped suffix the cluster has never granted) is
            // proposed by the drain's next pass and covered transitively by
            // one batch grant. Runs AFTER recovery so the residue round never
            // races a parked group's resolution.
            #[cfg(feature = "mirror-shipper")]
            if matches!(
                self.criome_gate.authorization(),
                crate::criome_gate::CriomeAuthorization::Enabled(_)
            ) {
                self.notify_head_advanced();
            }
        }
        output
    }

    /// Owner-only meta-socket `Import`: write pre-vetted records straight to the
    /// SEMA store with their given identifiers, bypassing the guardian admission
    /// pipeline entirely. This is the privileged restore/migration path — corpus
    /// rebuild, disaster recovery, machine-to-machine moves. The working signal
    /// stays fully gated; only the owner-only meta socket can import. It aborts
    /// to `Rejected` on the first store error so a partial import is loud, never
    /// silent.
    pub fn import(&mut self, request: ImportRequest) -> MetaOutput {
        let mut record_count: Integer = 0;
        for imported in request.into_payload().into_payload() {
            match self
                .nexus
                .store()
                .import_record(imported.record_identifier.into_payload(), imported.entry)
            {
                Ok(_) => record_count += 1,
                Err(_) => {
                    return MetaOutput::rejected(ConfigureRejection {
                        configure_rejection_reason: ConfigureRejectionReason::InternalError,
                        database_marker: self.nexus.database_marker(),
                    });
                }
            }
        }
        MetaOutput::imported(ImportReceipt {
            record_count: RecordCount::new(record_count),
            database_marker: self.nexus.database_marker(),
        })
    }

    pub async fn import_async(&mut self, request: ImportRequest) -> MetaOutput {
        self.import(request)
    }

    /// Owner-only meta `ObserveHead`: report the store's current versioned-log
    /// head — the content-addressed `EntryDigest` of the last durable entry,
    /// the SAME head `CriomeGate` captures via `LocalHeadCapture::spirit_head`
    /// (both in the `mirror-shipper`-gated `criome_gate` module) and authorizes
    /// for fan-out, and the SAME head the mirror durably lands.
    /// It is a pure read (no SEMA write, no guardian), so a guardian-required
    /// fail-closed daemon still answers it. The head is carried as a
    /// `HeadDigestHex` String — 64 lowercase hex
    /// characters — the exact form the router-forward-witness ingests as
    /// `HEAD_DIGEST_HEX`, and the SAME encoding criome's `ObjectDigest` carries.
    /// The hex comes from `EntryDigest`'s own `Display`, so spirit forwards its
    /// real content-addressing, not a re-hash. An empty log honestly reports no
    /// head (`None`).
    pub fn observe_head(&self) -> MetaOutput {
        match self.nexus.store().versioned_log_head() {
            Ok(head) => MetaOutput::head_observed(VersionedLogHead {
                database_marker: self.nexus.database_marker(),
                selected_head_digest: SelectedHeadDigest::new(
                    head.map(|digest| HeadDigestHex::new(digest.to_string())),
                ),
            }),
            Err(_) => MetaOutput::rejected(ConfigureRejection {
                configure_rejection_reason: ConfigureRejectionReason::InternalError,
                database_marker: self.nexus.database_marker(),
            }),
        }
    }

    pub async fn observe_head_async(&self) -> MetaOutput {
        self.observe_head()
    }

    /// Owner-only meta `ObserveHeadObject`: report the head entry's full wire
    /// BODY — the `rkyv` octets of the head `VersionedCommitLogEntry`, lowercase
    /// hex — beside the database marker. This is the exact body the production
    /// `ComponentShipper::envelope_for_entry` ships and the criome-auth forward
    /// carries (`versioned_log_head_object` reuses the same serialization), so
    /// the witness sources the REAL record body from Spirit instead of a
    /// placeholder. Surfaced as a hex `String` newtype, not `Bytes`, so the lean
    /// default meta-signal tree stays free of `nota` — the same constraint the
    /// sibling `HeadDigestHex` choice navigates. Decoding the hex and
    /// reconstructing through `VersionedCommitLogEntry::new` reproduces the
    /// `ObserveHead` digest, so head and body are consistent by construction.
    pub fn observe_head_object(&self) -> MetaOutput {
        match self.nexus.store().versioned_log_head_object() {
            Ok(object) => MetaOutput::head_object_observed(VersionedLogHeadObject {
                database_marker: self.nexus.database_marker(),
                selected_head_object: SelectedHeadObject::new(object.map(|octets| {
                    HeadObjectHex::new(
                        octets
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>(),
                    )
                })),
            }),
            Err(_) => MetaOutput::rejected(ConfigureRejection {
                configure_rejection_reason: ConfigureRejectionReason::InternalError,
                database_marker: self.nexus.database_marker(),
            }),
        }
    }

    pub async fn observe_head_object_async(&self) -> MetaOutput {
        self.observe_head_object()
    }

    /// Owner-only meta-socket `CollectRemovalCandidates`: archive every record
    /// matching the supplied query into the SEPARATE archive database at the
    /// owner-configured target, then physically retract it from the live log.
    /// This is the ONLY physical-deletion path; it runs with NO guardian, on the
    /// owner-only meta plane, mirroring `Import` — `&mut self` gives the
    /// schema-emitted `EngineActor` mailbox the same serialised exclusivity
    /// against every working write, without any component-internal lock. It
    /// reuses the store's `collect_removal_candidates` archive-then-retract
    /// primitive unchanged, and reports the archived / removed / skipped triple
    /// with the live database marker.
    pub fn collect_removal_candidates(
        &mut self,
        request: CollectRemovalCandidatesRequest,
    ) -> MetaOutput {
        match self
            .nexus
            .store()
            .collect_removal_candidates(request.into_payload())
        {
            Ok(removal_candidates_collection) => {
                MetaOutput::removal_candidates_collected(RemovalCandidatesCollectedReceipt {
                    removal_candidates_collection,
                    database_marker: self.nexus.database_marker(),
                })
            }
            Err(_) => MetaOutput::rejected(ConfigureRejection {
                configure_rejection_reason: ConfigureRejectionReason::InternalError,
                database_marker: self.nexus.database_marker(),
            }),
        }
    }

    pub async fn collect_removal_candidates_async(
        &mut self,
        request: CollectRemovalCandidatesRequest,
    ) -> MetaOutput {
        self.collect_removal_candidates(request)
    }

    pub fn intent_recorded_event(
        &self,
        record_identifier: &signal_schema::RecordIdentifier,
    ) -> Result<Option<IntentEvent>, StoreError> {
        self.nexus.intent_recorded_event(record_identifier)
    }

    pub async fn intent_recorded_event_async(
        &self,
        record_identifier: &signal_schema::RecordIdentifier,
    ) -> Result<Option<IntentEvent>, StoreError> {
        self.intent_recorded_event(record_identifier)
    }

    pub fn intent_clarified_event(
        &self,
        receipt: &signal_schema::ClarificationReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        self.nexus.intent_clarified_event(receipt)
    }

    pub async fn intent_clarified_event_async(
        &self,
        receipt: &signal_schema::ClarificationReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        self.intent_clarified_event(receipt)
    }

    pub fn intent_superseded_event(
        &self,
        receipt: &SupersessionReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        self.nexus.intent_superseded_event(receipt)
    }

    pub async fn intent_superseded_event_async(
        &self,
        receipt: &SupersessionReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        self.intent_superseded_event(receipt)
    }

    pub fn intent_retired_event(&self, receipt: &signal_schema::RetirementReceipt) -> IntentEvent {
        self.nexus.intent_retired_event(receipt)
    }
}

impl SignalAdmission {
    #[cfg(feature = "testing-trace")]
    pub fn with_trace(trace_log: TraceLog) -> Self {
        Self {
            trace_log,
            ..Self::default()
        }
    }

    pub fn start(&self) {
        #[cfg(feature = "testing-trace")]
        self.trace_signal_activation(SignalObjectName::Started);
    }

    pub fn stop(&self) {
        #[cfg(feature = "testing-trace")]
        self.trace_signal_activation(SignalObjectName::Stopped);
    }

    /// Admit a wire Input: mint the origin route, issue a message
    /// identifier, and validate against the schema-emitted rules.
    pub fn admit(&self, input: Input) -> Result<SignalAccepted, SignalRejected> {
        let origin_route = self.issue_origin_route();
        let identifier = self.issue_message_identifier();
        let short_header = input.short_header();
        if let Err(validation_error) = input.validate() {
            return Err(SignalRejected {
                origin_route,
                validation_error,
            });
        }
        #[cfg(feature = "testing-trace")]
        self.trace_signal_admitted();
        Ok(SignalAccepted {
            sent: MessageSent::new(identifier, origin_route, short_header),
            input,
            origin_route,
        })
    }

    pub fn triage(
        &self,
        input: Input,
        origin_route: OriginRoute,
    ) -> nexus_schema::nexus::Nexus<NexusWork> {
        #[cfg(not(feature = "testing-trace"))]
        let _ = self;
        #[cfg(feature = "testing-trace")]
        self.trace_signal_triaged();
        NexusWork::signal_arrived(input).with_origin_route(origin_route.into())
    }

    pub fn reply(&self, output: nexus_schema::nexus::Nexus<NexusAction>) -> SignalResponse<Output> {
        #[cfg(not(feature = "testing-trace"))]
        let _ = self;
        #[cfg(feature = "testing-trace")]
        self.trace_signal_replied();
        output.into_signal_output()
    }

    fn issue_message_identifier(&self) -> MessageIdentifier {
        let mut next = self
            .next_message_identifier
            .lock()
            .expect("message identifier lock");
        *next += 1;
        MessageIdentifier::new(*next)
    }

    fn issue_origin_route(&self) -> OriginRoute {
        let mut next = self.next_origin_route.lock().expect("origin route lock");
        *next += 1;
        OriginRoute::new(ORIGIN_ROUTE_BASE + *next)
    }

    #[cfg(feature = "testing-trace")]
    fn trace_signal_activation(&self, object_name: SignalObjectName) {
        self.trace_log
            .record(TraceEvent::new(ObjectName::Signal(object_name)));
    }

    #[cfg(feature = "testing-trace")]
    fn trace_signal_admitted(&self) {
        self.trace_signal_activation(SignalObjectName::Admitted);
    }

    #[cfg(feature = "testing-trace")]
    fn trace_signal_triaged(&self) {
        self.trace_signal_activation(SignalObjectName::Triaged);
    }

    #[cfg(feature = "testing-trace")]
    fn trace_signal_replied(&self) {
        self.trace_signal_activation(SignalObjectName::Replied);
    }

    #[cfg(feature = "testing-trace")]
    fn trace_signal_rejected(&self) {
        self.trace_signal_activation(SignalObjectName::Rejected);
    }
}

impl SignalAccepted {
    pub fn identifier(&self) -> MessageIdentifier {
        self.sent.identifier
    }

    pub fn message_sent(&self) -> &MessageSent {
        &self.sent
    }

    /// Run the validated mail through the SignalEngine + NexusEngine
    /// composition: triage Signal Input into Nexus Input, execute Nexus
    /// (which drives SEMA through `SemaEngine`), and frame the Nexus reply
    /// as Signal Output.
    ///
    /// The sent hook (the Signal→Nexus on_sent event) fires BEFORE the
    /// triage call, so an observer sees the handoff before any SEMA state
    /// changes. The processed hook fires after `NexusEngine::execute`
    /// returns and before Signal frames the reply. The `&mut Nexus`
    /// exclusive borrow held across `NexusEngine::execute` is the
    /// single-flight guard.
    pub async fn process_with(
        self,
        signal_admission: &SignalAdmission,
        nexus: &mut Nexus,
    ) -> SignalResponse<Output> {
        #[cfg(not(feature = "testing-trace"))]
        let _ = signal_admission;
        let identifier = self.identifier();
        self.sent
            .push_to(&mut nexus.mail_ledger().hook())
            .expect("spirit mail ledger is infallible");
        #[cfg(feature = "testing-trace")]
        signal_admission.trace_signal_triaged();
        let nexus_input =
            NexusWork::signal_arrived(self.input).with_origin_route(self.origin_route.into());
        let origin_route = nexus_input.origin_route();
        let nexus_output = nexus.execute_to_reply(nexus_input).await;
        let signal_output = nexus_output.into_signal_output();
        MessageProcessed::new(
            identifier,
            origin_route.into(),
            signal_output.root().clone(),
        )
        .push_to(&mut nexus.mail_ledger().hook())
        .expect("spirit mail ledger is infallible");
        #[cfg(feature = "testing-trace")]
        signal_admission.trace_signal_replied();
        signal_output
    }
}

impl MailLedger {
    pub fn hook(&self) -> MailLedgerHook<'_> {
        MailLedgerHook { ledger: self }
    }

    /// The currently in-flight sent markers. Terminal mail is reclaimed rather
    /// than retained as an unbounded history.
    pub fn events(&self) -> Vec<MailLedgerEvent> {
        let mut sent: Vec<_> = self
            .state
            .lock()
            .expect("mail ledger lock")
            .in_flight
            .values()
            .cloned()
            .collect();
        sent.sort_by_key(|mail| mail.mail_identifier.payload());
        sent.into_iter().map(MailLedgerEvent::sent).collect()
    }

    /// Total admitted sent messages. This aggregate metric is bounded state;
    /// individual terminal events are not retained.
    pub fn sent_message_count(&self) -> usize {
        self.state.lock().expect("mail ledger lock").sent_count
    }

    /// Total processed messages. This aggregate metric preserves the existing
    /// count surface without keeping completed mail events.
    pub fn processed_message_count(&self) -> usize {
        self.state.lock().expect("mail ledger lock").processed_count
    }

    pub fn in_flight_count(&self) -> usize {
        self.state.lock().expect("mail ledger lock").in_flight.len()
    }
}

impl MessageSentHook for MailLedgerHook<'_> {
    type Error = Infallible;

    fn message_sent(&mut self, event: MessageSent) -> Result<(), Self::Error> {
        let identifier = event.identifier.as_integer();
        let MailLedgerEvent::Sent(sent) = event.into_mail_ledger_event() else {
            unreachable!("a MessageSent event always projects to sent mail")
        };
        let mut state = self.ledger.state.lock().expect("mail ledger lock");
        state.sent_count = state.sent_count.saturating_add(1);
        state.in_flight.insert(identifier, sent);
        Ok(())
    }
}

impl MessageProcessedHook<Output> for MailLedgerHook<'_> {
    type Error = Infallible;

    fn message_processed(&mut self, event: MessageProcessed<Output>) -> Result<(), Self::Error> {
        let mut state = self.ledger.state.lock().expect("mail ledger lock");
        state.processed_count = state.processed_count.saturating_add(1);
        state.in_flight.remove(&event.identifier().as_integer());
        Ok(())
    }
}

impl nexus_schema::nexus::Nexus<NexusAction> {
    pub fn into_signal_output(self) -> SignalResponse<Output> {
        let origin_route = OriginRoute::from(self.origin_route());
        let root = match self.into_root() {
            NexusAction::ReplyToSignal(output) => output,
            _ => Output::error(ErrorReport::new(ErrorMessage::new(
                "nexus returned non-signal action",
            ))),
        };
        SignalResponse::new(origin_route, root)
    }
}

impl std::ops::Deref for crate::schema::sema::Recorded {
    type Target = SemaReceipt;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for crate::schema::sema::CertaintyChanged {
    type Target = signal_schema::CertaintyChangeReceipt;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for crate::schema::sema::RecordChanged {
    type Target = signal_schema::RecordChangeReceipt;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for crate::schema::sema::Observed {
    type Target = signal_schema::ObservedRecords;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for crate::schema::sema::Found {
    type Target = signal_schema::FoundRecord;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for crate::schema::sema::Counted {
    type Target = signal_schema::CountedRecords;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for nexus_schema::ChangeCertainty {
    type Target = signal_schema::CertaintyChange;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for nexus_schema::ChangeRecord {
    type Target = signal_schema::RecordChange;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl SignalRejected {
    pub fn into_signal_output(self) -> SignalResponse<Output> {
        SignalResponse::new(
            self.origin_route,
            self.validation_error.into_signal_output(),
        )
    }
}
