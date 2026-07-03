use std::{convert::Infallible, sync::Mutex as StdMutex};

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
            self as signal_schema, ApplyRefusalReason, AuthorizedRecordApplication, DatabaseMarker,
            ErrorMessage, Input, Integer, IntentEvent, Output, RecordCount, SemaReceipt,
            SupersessionReceipt, ValidationError,
        },
    },
    store::{Store, StoreError},
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, TraceEvent, TraceLog};

const ORIGIN_ROUTE_BASE: Integer = 1_000_000;

/// The composed failure modes of the gated head fan-out
/// ([`Engine::gate_and_ship_head`]): a head-capture / store read failure, a
/// gate-machinery failure (the blocking criome task or an off-contract reply),
/// or a mirror-shipper transport failure on the authorized ship. An
/// UNCONFIGURED, DENIED, or UNREACHABLE criome is NOT an error — it is a
/// [`crate::criome_gate::GateDecision`] the daemon handles by holding the head
/// back.
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
    // The 1-of-1 LOCAL criome gate (Spirit `xhwa`). Present only under the
    // `mirror-shipper` feature alongside the shipper it gates. Unarmed by
    // default: a daemon with no configured local criome holds the head back
    // because no authorization exists. When armed, every fan-out is held
    // behind a real criome authorization round-trip.
    #[cfg(feature = "mirror-shipper")]
    criome_gate: crate::criome_gate::CriomeGate,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
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

#[derive(Debug, Default)]
pub struct MailLedger {
    events: StdMutex<Vec<MailLedgerEvent>>,
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
                #[cfg(feature = "mirror-shipper")]
                criome_gate: crate::criome_gate::CriomeGate::new(),
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
            #[cfg(feature = "mirror-shipper")]
            criome_gate: crate::criome_gate::CriomeGate::new(),
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
        #[cfg(all(feature = "agent-guardian", feature = "mirror-shipper"))]
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

    /// The current versioned-log head `EntryDigest` after the latest local
    /// commit — the content-addressed head `D` the criome gate authorizes
    /// BEFORE fan-out. Read from the local log, never from `ShipOutcome.head`
    /// (which exists only after a ship). `None` when nothing has been
    /// committed to the versioned log yet (an empty store cannot be authorized
    /// and is not shipped).
    #[cfg(feature = "mirror-shipper")]
    pub fn versioned_log_head(&self) -> Result<Option<sema_engine::EntryDigest>, StoreError> {
        self.nexus.store().versioned_log_head()
    }

    #[cfg(feature = "agent-guardian")]
    pub fn guardian_decision_count(&self) -> usize {
        self.nexus.store().guardian_decision_count().unwrap_or(0)
    }

    /// The intent-guardian system prompt the installed guardian will currently
    /// send, or `None` when no guardian is installed. Reflects the live prompt
    /// source after any owner `Configure` swap.
    #[cfg(feature = "agent-guardian")]
    pub fn guardian_intent_system_prompt(&self) -> Option<String> {
        self.nexus.guardian_intent_system_prompt()
    }

    pub fn sent_message_count(&self) -> usize {
        self.nexus.mail_ledger().sent_message_count()
    }

    pub fn processed_message_count(&self) -> usize {
        self.nexus.mail_ledger().processed_message_count()
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
        // Swap the live guardian's role prompt when the owner supplies a target.
        // `Default` restores the compiled-in (acknowledged strict-bar) role;
        // `Prompt` installs an owner-supplied role override. An absent target
        // leaves the live guardian's current prompt unchanged. Like the other
        // Configure targets this is runtime policy, not durable state: a
        // restarted daemon starts on the compiled-in role until the owner
        // re-sends `Configure`. Only present under `agent-guardian`; the default
        // build echoes the target in the receipt with no guardian to apply it.
        #[cfg(feature = "agent-guardian")]
        if let Some(guardian_prompt_target) = guardian_prompt_target.as_ref() {
            let prompt_source = match guardian_prompt_target {
                crate::schema::meta_signal::GuardianPromptTarget::Default => {
                    crate::guardian_prompt::GuardianPromptSource::compiled_in()
                }
                crate::schema::meta_signal::GuardianPromptTarget::Prompt(prompt) => {
                    crate::guardian_prompt::GuardianPromptSource::role_override(
                        prompt.payload().payload().clone(),
                    )
                }
            };
            self.nexus.set_guardian_prompt_source(prompt_source);
        }
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
        #[cfg(feature = "mirror-shipper")]
        match &criome_gate_target {
            Some(crate::schema::meta_signal::CriomeGateTarget::Socket(socket_path)) => {
                self.criome_gate
                    .configure_socket(socket_path.payload().payload().clone());
                #[cfg(feature = "agent-guardian")]
                self.nexus
                    .configure_operation_authorizer(socket_path.payload().payload().clone());
            }
            Some(crate::schema::meta_signal::CriomeGateTarget::Default) | None => {
                self.criome_gate.clear();
                #[cfg(feature = "agent-guardian")]
                self.nexus.clear_operation_authorizer();
            }
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

    /// Arm the 1-of-1 LOCAL criome gate against a local criome socket path and
    /// the deploy-config attestor (the admitted contract + signed evidence). An
    /// armed gate holds every fan-out behind a criome authorization round-trip;
    /// an unarmed gate holds the head back because no authorization exists.
    #[cfg(feature = "mirror-shipper")]
    pub fn arm_criome_gate(
        &mut self,
        socket: impl Into<std::path::PathBuf>,
        attestor: crate::criome_gate::SpiritAttestor,
    ) {
        self.criome_gate.arm(socket, attestor);
    }

    /// Whether the 1-of-1 LOCAL criome gate is armed.
    #[cfg(feature = "mirror-shipper")]
    pub fn criome_gate_armed(&self) -> bool {
        self.criome_gate.is_armed()
    }

    /// THE GATE. Capture the post-commit local head `D` and apply the configured
    /// authorization mode.
    ///
    /// Order (report 703-6 Item 1): the working write already committed
    /// LOCALLY before this runs (`Engine::handle_async`). `Gating` mode asks
    /// local criome over the per-user Unix socket and ships only when criome
    /// authorizes. `Observing` mode emits the same request and proceeds without
    /// waiting for criome's verdict.
    ///
    /// Returns the gate decision: `Authorized` carries the emitted reference
    /// (the SAME projection that fed the criome request, so authorized head ==
    /// fanned head). An empty versioned log (nothing to authorize) is a no-op:
    /// it returns `None`.
    #[cfg(feature = "mirror-shipper")]
    pub async fn gate_and_ship_head(
        &self,
    ) -> Result<Option<crate::criome_gate::GateDecision>, GateAndShipError> {
        let Some(head_digest) = self.versioned_log_head()? else {
            return Ok(None);
        };
        let capture = crate::criome_gate::LocalHeadCapture::spirit_head(head_digest);
        let decision = match self.authorization_mode {
            signal_spirit::AuthorizationMode::Gating => {
                self.criome_gate.authorize_head(&capture).await?
            }
            signal_spirit::AuthorizationMode::Observing => {
                let decision = self.criome_gate.observe_authorization(&capture).await?;
                #[cfg(feature = "testing-trace")]
                if matches!(decision, crate::criome_gate::GateDecision::Observed(_)) {
                    self.trace_log
                        .record(TraceEvent::new(ObjectName::Authorization(
                            crate::AuthorizationObjectName::Observed,
                        )));
                }
                decision
            }
        };
        if decision.ships() {
            self.mirror_shipper.ship_unshipped().await?;
        }
        Ok(Some(decision))
    }

    /// Publish the store's latest local checkpoint to the configured mirror,
    /// the portable restore body a fresh store fetches alongside the shipped
    /// log suffix. A no-op when mirroring is off.
    #[cfg(feature = "mirror-shipper")]
    pub async fn publish_checkpoint_to_mirror(&self) -> Result<bool, MirrorShipperError> {
        self.mirror_shipper.publish_checkpoint().await
    }

    pub async fn configure_async(&mut self, request: ConfigureRequest) -> MetaOutput {
        self.configure(request)
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

    /// The quorum-gated authorized-apply ingress (Spirit `xhwa`, piece 4): land
    /// an arriving record LIVE into the running store after the LOCAL criome
    /// re-judges the carried Evidence, fail-closed.
    ///
    /// Unlike the owner-only meta `Import` (which applies on owner-trust), this
    /// working-tier ingress applies a foreign record ONLY because the quorum
    /// authorized it — confirmed here, independently, by this node's criome from
    /// the carried Evidence, never on the sender's say-so. The parse verifies the
    /// carried entry re-hashes to the digest the Evidence authorized (content
    /// binding); the criome re-judge must return `Authorized`; only then does the
    /// record land via `Store::import_record` into the same live
    /// `Arc<SemaDatabase>` reads see, so a following `Observe` returns it with no
    /// restart. Any malformed body, re-hash mismatch, unreachable/unconfigured
    /// criome, or non-`Authorized` verdict refuses fail-closed.
    #[cfg(feature = "mirror-shipper")]
    pub async fn apply_authorized_record(&self, request: AuthorizedRecordApplication) -> Output {
        use crate::apply_ingress::{AuthorizedApplyOutcome, PreparedAuthorizedApply};
        let prepared = match PreparedAuthorizedApply::prepare(
            &request,
            self.criome_gate.configured_contract(),
        ) {
            Ok(prepared) => prepared,
            Err(reason) => return AuthorizedApplyOutcome::Refused(reason).into_output(),
        };
        let decision = match self
            .criome_gate
            .evaluate_carried(prepared.evaluation().clone())
            .await
        {
            Ok(Some(decision)) => decision,
            // Unarmed / unreachable criome, or a gate-machinery fault: no
            // authorization exists, so the apply is held back, fail-closed.
            Ok(None) | Err(_) => {
                return AuthorizedApplyOutcome::Refused(
                    ApplyRefusalReason::AuthorizationUnavailable,
                )
                .into_output();
            }
        };
        prepared
            .resolve(&decision, self.nexus.store())
            .into_output()
    }

    /// The fail-closed default when the mirror surface is not built in: a daemon
    /// without the local criome gate cannot authorize a foreign apply, so it
    /// refuses. Keeps the working `Input::ApplyAuthorizedRecord` variant total
    /// across builds.
    #[cfg(not(feature = "mirror-shipper"))]
    pub async fn apply_authorized_record(&self, request: AuthorizedRecordApplication) -> Output {
        let _ = request;
        Output::apply_refused(signal_schema::ApplyRefusal::new(
            ApplyRefusalReason::AuthorizationUnavailable,
        ))
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

    pub fn events(&self) -> Vec<MailLedgerEvent> {
        self.events.lock().expect("mail ledger lock").clone()
    }

    pub fn sent_message_count(&self) -> usize {
        self.events
            .lock()
            .expect("mail ledger lock")
            .iter()
            .filter(|event| event.is_sent())
            .count()
    }

    pub fn processed_message_count(&self) -> usize {
        self.events
            .lock()
            .expect("mail ledger lock")
            .iter()
            .filter(|event| event.is_processed())
            .count()
    }
}

impl MessageSentHook for MailLedgerHook<'_> {
    type Error = Infallible;

    fn message_sent(&mut self, event: MessageSent) -> Result<(), Self::Error> {
        self.ledger
            .events
            .lock()
            .expect("mail ledger lock")
            .push(event.into_mail_ledger_event());
        Ok(())
    }
}

impl MessageProcessedHook<Output> for MailLedgerHook<'_> {
    type Error = Infallible;

    fn message_processed(&mut self, event: MessageProcessed<Output>) -> Result<(), Self::Error> {
        self.ledger
            .events
            .lock()
            .expect("mail ledger lock")
            .push(event.processed_mail_event());
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
