use std::{convert::Infallible, sync::Mutex as StdMutex};

#[cfg(feature = "mirror-shipper")]
use crate::shipper::{MirrorShipper, MirrorShipperError};
use crate::{
    nexus::Nexus,
    schema::{
        meta_signal::{
            ConfigureReceipt, ConfigureRejection, ConfigureRejectionReason, ConfigureRequest,
            ImportReceipt, ImportRequest, Output as MetaOutput,
        },
        nexus::{self as nexus_schema, NexusAction, NexusEffectCommand, NexusEngine, NexusWork},
        sema::ErrorReport,
        signal::{
            self as signal_schema, CertaintySelection, DatabaseMarker, Description, Domain,
            DomainMatch, DomainScope, DomainScopes, Domains, EngineStartFailure, EngineStopFailure,
            Entry, ErrorMessage, ImportanceSelection, Input, Integer, IntentEvent, KeywordMatch,
            MailIdentifier, MailLedgerEvent, MessageIdentifier, MessageProcessed,
            MessageProcessedHook, MessageSent, MessageSentHook, OriginRoute, Output, Privacy,
            PrivacySelection, ProcessedMail, Query, RecordCount, RecordSelection,
            ReferentSelection, ScopeSet, SemaReceipt, SentMail, ShortHeader, SignalEngine,
            SignalRejection, StatementText, SupersessionReceipt, TextMatch, ValidationError,
        },
    },
    store::{Store, StoreError},
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, TraceEvent, TraceLog, schema::signal::SignalObjectName};

const ORIGIN_ROUTE_BASE: Integer = 1_000_000;

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
    // The OFF-by-default mirror gate. Present only under the `mirror-shipper`
    // feature (which the binary-only daemon build excludes to keep nota-next
    // out of its dependency tree). Even when built in, it is unarmed until an
    // owner `Configure` carries a `MirrorTarget::Address`, so a daemon that
    // never receives a mirror target behaves identically to one built before
    // mirroring existed.
    #[cfg(feature = "mirror-shipper")]
    mirror_shipper: MirrorShipper,
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
    input: signal_schema::signal::Signal<Input>,
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
                #[cfg(feature = "mirror-shipper")]
                mirror_shipper: MirrorShipper::new(),
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
            #[cfg(feature = "mirror-shipper")]
            mirror_shipper: MirrorShipper::new(),
            trace_log,
        }
    }

    #[cfg(feature = "testing-trace")]
    pub fn trace_events(&self) -> Vec<TraceEvent> {
        self.trace_log.events()
    }

    pub fn start(&mut self) -> Result<(), EngineStartFailure> {
        NexusEngine::on_start(&mut self.nexus)?;
        self.signal_admission.start()
    }

    pub fn stop(&mut self) -> Result<(), EngineStopFailure> {
        self.signal_admission.stop()?;
        NexusEngine::on_stop(&mut self.nexus)?;
        Ok(())
    }

    /// Run one request through Signal admission, the NexusEngine
    /// composition, and the durable SEMA store.
    ///
    /// Signal admits the input (mints the origin route, issues an
    /// identifier, and validates) before any deeper layer sees it. The
    /// sent hook fires at the Signal→Nexus handoff; the processed hook
    /// fires after the NexusEngine returns its reply.
    pub fn handle(&mut self, input: Input) -> signal_schema::signal::Signal<Output> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("spirit sync handle runtime")
            .block_on(self.handle_async(input))
    }

    pub async fn handle_async(&mut self, input: Input) -> signal_schema::signal::Signal<Output> {
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

    #[cfg(feature = "agent-guardian")]
    pub fn guardian_decision_count(&self) -> usize {
        self.nexus.store().guardian_decision_count().unwrap_or(0)
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
        let ConfigureRequest {
            archive_database_target,
            mirror_target,
        } = request;
        self.nexus
            .set_archive_target(archive_database_target.clone());
        self.nexus.set_mirror_target(mirror_target.clone());
        // Arm (or disarm) the mirror shipper against the live engine handle.
        // A bad mirror address is an owner-config error, not a SEMA fault, so
        // it rejects the Configure rather than silently leaving mirroring off.
        // Present only when the gated shipper is built in; without it the meta
        // target is still recorded on the store but no shipper exists.
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
        MetaOutput::configured(ConfigureReceipt {
            archive_database_target,
            mirror_target,
            database_marker: self.nexus.database_marker(),
        })
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

    pub fn start(&self) -> Result<(), EngineStartFailure> {
        #[cfg(feature = "testing-trace")]
        self.trace_signal_activation(SignalObjectName::Started);
        Ok(())
    }

    pub fn stop(&self) -> Result<(), EngineStopFailure> {
        #[cfg(feature = "testing-trace")]
        self.trace_signal_activation(SignalObjectName::Stopped);
        Ok(())
    }

    /// Admit a wire Input: mint the origin route, issue a message
    /// identifier, and validate against the schema-emitted rules.
    pub fn admit(&self, input: Input) -> Result<SignalAccepted, SignalRejected> {
        let origin_route = self.issue_origin_route();
        let signal_input = input.with_origin_route(origin_route);
        let identifier = self.issue_message_identifier();
        if let Err(validation_error) = signal_input.root().validate() {
            return Err(SignalRejected {
                origin_route,
                validation_error,
            });
        }
        #[cfg(feature = "testing-trace")]
        self.trace_signal_admitted();
        Ok(SignalAccepted {
            sent: signal_input.message_sent(identifier),
            input: signal_input,
        })
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
}

impl SignalEngine for SignalAdmission {
    type NexusInput = nexus_schema::nexus::Nexus<NexusWork>;
    type NexusOutput = nexus_schema::nexus::Nexus<NexusAction>;

    fn on_start(&mut self) -> Result<(), EngineStartFailure> {
        self.start()
    }

    fn on_stop(&mut self) -> Result<(), EngineStopFailure> {
        self.stop()
    }

    #[cfg(feature = "testing-trace")]
    fn trace_signal_activation(&self, object_name: SignalObjectName) {
        self.trace_log
            .record(TraceEvent::new(ObjectName::Signal(object_name)));
    }

    fn triage_inner(
        &self,
        input: signal_schema::signal::Signal<Input>,
    ) -> nexus_schema::nexus::Nexus<NexusWork> {
        let origin_route = input.origin_route();
        NexusWork::signal_arrived(input.into_root()).with_origin_route(origin_route.into())
    }

    fn reply_inner(
        &self,
        output: nexus_schema::nexus::Nexus<NexusAction>,
    ) -> signal_schema::signal::Signal<Output> {
        output.into_signal_output()
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
    pub async fn process_with<Signal>(
        self,
        signal_engine: &Signal,
        nexus: &mut Nexus,
    ) -> signal_schema::signal::Signal<Output>
    where
        Signal: SignalEngine<
                NexusInput = nexus_schema::nexus::Nexus<NexusWork>,
                NexusOutput = nexus_schema::nexus::Nexus<NexusAction>,
            >,
    {
        self.sent
            .push_to(&mut nexus.mail_ledger().hook())
            .expect("spirit mail ledger is infallible");
        let identifier = self.identifier();
        let nexus_input = signal_engine.triage(self.input);
        let origin_route = nexus_input.origin_route();
        let nexus_output = nexus.execute_to_reply(nexus_input).await;
        let signal_output = signal_engine.reply(nexus_output);
        MessageProcessed::new(
            identifier,
            origin_route.into(),
            signal_output.root().clone(),
        )
        .push_to(&mut nexus.mail_ledger().hook())
        .expect("spirit mail ledger is infallible");
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

impl Input {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::State(statement) => statement.payload().validate(),
            Self::Record(record) => record.payload().validate(),
            Self::Propose(propose) => propose.payload().validate(),
            Self::Clarify(clarify) => clarify.payload().validate(),
            Self::Supersede(supersede) => supersede.payload().validate(),
            Self::Observe(observe) => observe.payload().validate(),
            Self::PublicRecords(selection) => selection.payload().validate(),
            Self::PrivateRecords(selection) => selection.payload().validate(),
            Self::Lookup(_)
            | Self::ChangeCertainty(_)
            | Self::BumpImportance(_)
            | Self::LookupStash(_)
            | Self::Tap(_)
            | Self::Untap(_)
            | Self::Version
            | Self::Marker => Ok(()),
            Self::ChangeRecord(change) => change.payload().validate(),
            Self::Remove(remove) => remove.payload().validate(),
            Self::RegisterReferent(register) => register.payload().validate(),
            Self::Retire(retire) => retire.payload().validate(),
            Self::CollectRemovalCandidates(collection) => collection.payload().validate(),
            Self::SubscribeIntent(query) => query.payload().validate(),
            Self::Count(count) => count.payload().validate(),
        }
    }
}

impl crate::schema::signal::Statement {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.payload().trim().is_empty() {
            return Err(ValidationError::EmptyDescription);
        }
        Ok(())
    }
}

impl Entry {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.domains.is_empty() {
            return Err(ValidationError::EmptyDomain);
        }
        if self.description.trim().is_empty() {
            return Err(ValidationError::EmptyDescription);
        }
        Ok(())
    }
}

impl crate::schema::signal::Justification {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.reasoning.payload().trim().is_empty() {
            return Err(ValidationError::EmptyDescription);
        }
        for quote in self.testimony.payload() {
            if quote.quote_text.payload().trim().is_empty() {
                return Err(ValidationError::EmptyDescription);
            }
            if quote
                .antecedent
                .as_ref()
                .is_some_and(|antecedent| antecedent.payload().trim().is_empty())
            {
                return Err(ValidationError::EmptyDescription);
            }
        }
        Ok(())
    }
}

impl crate::schema::signal::RecordRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.entry.validate()?;
        self.justification.validate()
    }
}

impl crate::schema::signal::Proposal {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.entry.validate()?;
        self.justification.validate()
    }
}

impl crate::schema::signal::Referent {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.payload().trim().is_empty() {
            return Err(ValidationError::EmptyQueryReferent);
        }
        Ok(())
    }
}

impl crate::schema::signal::Referents {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self
            .payload()
            .iter()
            .any(|referent| referent.payload().trim().is_empty())
        {
            return Err(ValidationError::EmptyQueryReferent);
        }
        Ok(())
    }
}

impl crate::schema::signal::ReferentRegistration {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.referent.validate()?;
        self.aliases.validate()?;
        self.justification.validate()
    }
}

impl crate::schema::signal::Removal {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.justification.validate()
    }
}

impl crate::schema::signal::RecordChange {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.entry.validate()?;
        self.justification.validate()
    }
}

impl crate::schema::signal::Clarification {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.description.trim().is_empty() {
            return Err(ValidationError::EmptyDescription);
        }
        self.justification.validate()
    }
}

impl crate::schema::signal::Supersession {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.retired_identifiers.payload().is_empty() {
            return Err(ValidationError::EmptyDescription);
        }
        if self.replacements.payload().is_empty() {
            return Err(ValidationError::EmptyDescription);
        }
        for replacement in self.replacements.payload() {
            replacement.validate()?;
        }
        self.justification.validate()
    }
}

impl crate::schema::signal::Retirement {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.justification.validate()
    }
}

impl Query {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.domain_match.validate()?;
        self.keyword_match.validate()?;
        self.text_match.validate()?;
        self.referent_selection.validate()
    }
}

impl crate::schema::signal::RemovalCandidateCollection {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.record_query.validate()?;
        self.justification.validate()
    }
}

impl crate::schema::signal::RecordQuery {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.payload().validate()
    }
}

impl RecordSelection {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.domain_match.validate()
    }

    pub fn into_public_query(self) -> Query {
        Query {
            domain_match: self.domain_match,
            keyword_match: KeywordMatch::Any,
            text_match: TextMatch::Any,
            referent_selection: ReferentSelection::Any,
            kind: self.kind,
            privacy_selection: PrivacySelection::default_observation_privacy(),
            certainty_selection: CertaintySelection::default_observation_certainty(),
            importance_selection: ImportanceSelection::default_observation_importance(),
        }
    }

    pub fn into_private_query(self) -> Query {
        Query {
            domain_match: self.domain_match,
            keyword_match: KeywordMatch::Any,
            text_match: TextMatch::Any,
            referent_selection: ReferentSelection::Any,
            kind: self.kind,
            privacy_selection: PrivacySelection::at_least(Privacy::new(
                PrivacySelection::private_floor(),
            )),
            certainty_selection: CertaintySelection::default_observation_certainty(),
            importance_selection: ImportanceSelection::default_observation_importance(),
        }
    }
}

impl DomainMatch {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Any => Ok(()),
            Self::Partial(scopes) => {
                if scopes.payload().is_empty() {
                    return Err(ValidationError::EmptyQueryDomain);
                }
                Ok(())
            }
            Self::Full(scopes) => {
                if scopes.payload().is_empty() {
                    return Err(ValidationError::EmptyQueryDomain);
                }
                Ok(())
            }
        }
    }

    pub fn matches(&self, entry_domains: &Domains) -> bool {
        match self {
            Self::Any => true,
            Self::Partial(partial) => partial.payload().matches_any_domain(entry_domains),
            Self::Full(full) => full
                .payload()
                .iter()
                .all(|scope| scope.expand().matches_any_domain(entry_domains)),
        }
    }
}

impl KeywordMatch {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Any => Ok(()),
            Self::AnyKeyword(keywords) => keywords.payload().validate(),
            Self::AllKeywords(keywords) => keywords.payload().validate(),
        }
    }
}

impl TextMatch {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Any => Ok(()),
            Self::ContainsText(search_text) => {
                if search_text.payload().payload().trim().is_empty() {
                    return Err(ValidationError::EmptySearchText);
                }
                Ok(())
            }
        }
    }
}

impl ReferentSelection {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Any => Ok(()),
            Self::AnyReferent(referents) => {
                if referents.payload().payload().is_empty() {
                    return Err(ValidationError::EmptyQueryReferent);
                }
                Ok(())
            }
            Self::AllReferents(referents) => {
                if referents.payload().payload().is_empty() {
                    return Err(ValidationError::EmptyQueryReferent);
                }
                Ok(())
            }
        }
    }
}

impl signal_schema::Keywords {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.payload().is_empty()
            || self
                .payload()
                .iter()
                .any(|keyword| keyword.payload().trim().is_empty())
        {
            return Err(ValidationError::EmptyKeyword);
        }
        Ok(())
    }
}

impl PrivacySelection {
    pub fn private_floor() -> signal_schema::Magnitude {
        signal_schema::Magnitude::Minimum
    }
}

impl MessageIdentifier {
    pub fn as_integer(&self) -> Integer {
        self.payload()
    }
}

impl MessageSent {
    pub fn into_mail_ledger_event(self) -> MailLedgerEvent {
        MailLedgerEvent::sent(SentMail {
            mail_identifier: MailIdentifier::new(self.identifier.as_integer()),
            origin_route: self.origin_route(),
            short_header: ShortHeader::new(self.short_header),
        })
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
    pub fn is_sent(&self) -> bool {
        matches!(self, Self::Sent(_))
    }

    pub fn is_processed(&self) -> bool {
        matches!(self, Self::Processed(_))
    }
}

impl DatabaseMarker {
    pub fn zero() -> Self {
        Self {
            commit_sequence: signal_schema::CommitSequence::new(0),
            state_digest: signal_schema::StateDigest::new(0),
        }
    }
}

impl ValidationError {
    pub fn into_signal_output(self) -> Output {
        Output::rejected(SignalRejection::new(self))
    }
}

impl nexus_schema::nexus::Nexus<NexusAction> {
    pub fn into_signal_output(self) -> signal_schema::signal::Signal<Output> {
        let origin_route = self.origin_route();
        match self.into_root() {
            NexusAction::ReplyToSignal(output) => {
                output.into_payload().with_origin_route(origin_route.into())
            }
            _ => Output::error(ErrorReport::new(ErrorMessage::new(
                "nexus returned non-signal action",
            )))
            .with_origin_route(origin_route.into()),
        }
    }
}

impl Domains {
    pub fn from_strings(labels: Vec<String>) -> Self {
        let mut domains = Vec::new();
        for label in labels {
            Domain::push_from_label(&label, &mut domains);
        }
        Self::new(domains)
    }

    pub fn is_empty(&self) -> bool {
        self.payload().is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Domain> {
        self.payload().iter()
    }

    pub fn matches_scope_set(&self, scope_set: &ScopeSet) -> bool {
        self.iter().any(|domain| scope_set.matches_domain(domain))
    }
}

impl DomainScopes {
    pub fn from_strings(labels: Vec<String>) -> Self {
        Self::from_domains(&Domains::from_strings(labels))
    }

    pub fn from_domains(domains: &Domains) -> Self {
        Self::new(domains.iter().cloned().map(DomainScope::from).collect())
    }

    pub fn is_empty(&self) -> bool {
        self.payload().is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &DomainScope> {
        self.payload().iter()
    }

    pub fn matches_any_domain(&self, domains: &Domains) -> bool {
        self.iter()
            .any(|scope| scope.expand().matches_any_domain(domains))
    }
}

impl DomainScope {
    pub fn matches_domain(&self, domain: &Domain) -> bool {
        self.contains_domain(domain)
    }
}

impl ScopeSet {
    pub fn iter(&self) -> impl Iterator<Item = &DomainScope> {
        self.payload().iter()
    }

    pub fn matches_domain(&self, domain: &Domain) -> bool {
        self.iter().any(|scope| scope.matches_domain(domain))
    }

    pub fn matches_any_domain(&self, domains: &Domains) -> bool {
        domains.matches_scope_set(self)
    }
}

impl Domain {
    pub fn matches_scope(&self, scope: &DomainScope) -> bool {
        scope.contains_domain(self)
    }

    fn software(payload: signal_schema::Software) -> Self {
        Self::Technology(signal_schema::Technology::Software(payload))
    }

    fn push_from_label(label: &str, domains: &mut Vec<Self>) {
        let label = label.trim().to_ascii_lowercase();
        if label.is_empty() {
            return;
        }
        if Self::contains_any(
            &label,
            &["schema", "record-shape", "type-definition", "data-model"],
        ) {
            Self::push_unique(domains, Self::schema());
        }
        if Self::contains_any(&label, &["nota", "syntax", "delimiter", "parser"]) {
            Self::push_unique(domains, Self::notation());
        }
        if Self::contains_any(&label, &["language", "naming", "vocabulary"]) {
            Self::push_unique(domains, Self::terminology());
        }
        if Self::contains_any(&label, &["rust", "code", "program", "programming"]) {
            Self::push_unique(domains, Self::programming());
        }
        if Self::contains_any(&label, &["test", "fixture", "trace", "verification"]) {
            Self::push_unique(domains, Self::testing());
        }
        if Self::contains_any(
            &label,
            &[
                "signal",
                "nexus",
                "sema",
                "component",
                "daemon",
                "runtime",
                "architecture",
                "build",
                "forge",
            ],
        ) {
            Self::push_unique(domains, Self::architecture());
        }
        if Self::contains_any(
            &label,
            &[
                "workspace",
                "orchestrate",
                "workflow",
                "role",
                "report",
                "bead",
                "discipline",
                "intent",
                "privacy",
                "capture",
            ],
        ) {
            Self::push_unique(domains, Self::documentation());
        }
        if Self::contains_any(
            &label,
            &[
                "agent", "persona", "llm", "harness", "subagent", "model", "mail",
            ],
        ) {
            Self::push_unique(domains, Self::intelligence());
        }
        if Self::contains_any(
            &label,
            &[
                "cloud",
                "deploy",
                "nix",
                "cluster",
                "network",
                "secret",
                "production",
                "upgrade",
            ],
        ) {
            Self::push_unique(domains, Self::infrastructure());
        }
        if Self::contains_any(&label, &["assistant", "counselor", "personal", "health"]) {
            Self::push_unique(domains, Self::wellbeing());
        }
        if Self::contains_any(&label, &["governing", "governance", "policy"]) {
            Self::push_unique(domains, Self::policy());
        }
        if Self::contains_any(&label, &["relating", "relationship", "friendship"]) {
            Self::push_unique(domains, Self::rapport());
        }
        if domains.is_empty() {
            Self::push_unique(domains, Self::documentation());
        }
    }

    fn schema() -> Self {
        Self::software(signal_schema::Software::Data(
            signal_schema::Data::SchemaEvolution,
        ))
    }

    fn notation() -> Self {
        Self::Language(signal_schema::Language::Notation)
    }

    fn terminology() -> Self {
        Self::Language(signal_schema::Language::Terminology)
    }

    fn programming() -> Self {
        Self::software(signal_schema::Software::Languages(
            signal_schema::Languages::ProgrammingLanguages,
        ))
    }

    fn testing() -> Self {
        Self::software(signal_schema::Software::Quality(
            signal_schema::Quality::Testing,
        ))
    }

    fn architecture() -> Self {
        Self::software(signal_schema::Software::Engineering(
            signal_schema::Engineering::SoftwareArchitecture,
        ))
    }

    fn documentation() -> Self {
        Self::Information(signal_schema::Information::Documentation)
    }

    fn intelligence() -> Self {
        Self::software(signal_schema::Software::Intelligence(
            signal_schema::Intelligence::AgentSystems,
        ))
    }

    fn infrastructure() -> Self {
        Self::software(signal_schema::Software::Operations(
            signal_schema::Operations::InfrastructureAsCode,
        ))
    }

    fn wellbeing() -> Self {
        Self::Selfhood(signal_schema::Selfhood::Wellbeing)
    }

    fn policy() -> Self {
        Self::Governance(signal_schema::Governance::Policy)
    }

    fn rapport() -> Self {
        Self::Kinship(signal_schema::Kinship::Rapport)
    }

    fn contains_any(label: &str, needles: &[&str]) -> bool {
        needles.iter().any(|needle| label.contains(needle))
    }

    fn push_unique(domains: &mut Vec<Self>, domain: Self) {
        if !domains.contains(&domain) {
            domains.push(domain);
        }
    }
}

impl std::ops::Deref for signal_schema::RecordIdentifier {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::fmt::Display for signal_schema::RecordIdentifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.payload().fmt(formatter)
    }
}

impl Description {
    pub fn trim(&self) -> &str {
        self.payload().trim()
    }
}

impl StatementText {
    pub fn trim(&self) -> &str {
        self.payload().trim()
    }
}

impl Privacy {
    pub fn rank(&self) -> u64 {
        self.payload().rank()
    }
}

impl PartialEq<&str> for Description {
    fn eq(&self, other: &&str) -> bool {
        self.payload() == other
    }
}

impl PartialEq<&str> for ErrorMessage {
    fn eq(&self, other: &&str) -> bool {
        self.payload() == other
    }
}

impl PartialEq<&str> for signal_schema::Statement {
    fn eq(&self, other: &&str) -> bool {
        self.payload().payload() == other
    }
}

impl PartialEq<signal_schema::Magnitude> for Privacy {
    fn eq(&self, other: &signal_schema::Magnitude) -> bool {
        self.payload() == other
    }
}

impl PartialEq<signal_schema::Magnitude> for signal_schema::Certainty {
    fn eq(&self, other: &signal_schema::Magnitude) -> bool {
        self.payload() == other
    }
}

impl PartialEq<u64> for signal_schema::RecordCount {
    fn eq(&self, other: &u64) -> bool {
        self.payload() == other
    }
}

impl PartialEq<u64> for signal_schema::StashHandle {
    fn eq(&self, other: &u64) -> bool {
        self.payload() == other
    }
}

impl PartialEq<u64> for signal_schema::SubscriptionToken {
    fn eq(&self, other: &u64) -> bool {
        self.payload() == other
    }
}

impl PartialOrd<u64> for signal_schema::SubscriptionToken {
    fn partial_cmp(&self, other: &u64) -> Option<std::cmp::Ordering> {
        self.payload().partial_cmp(other)
    }
}

impl std::fmt::Display for signal_schema::StashHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.payload().fmt(formatter)
    }
}

impl PartialEq<u64> for signal_schema::CommitSequence {
    fn eq(&self, other: &u64) -> bool {
        self.payload() == other
    }
}

impl PartialOrd for signal_schema::CommitSequence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.payload().partial_cmp(other.payload())
    }
}

impl std::fmt::Display for signal_schema::CommitSequence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.payload().fmt(formatter)
    }
}

impl PartialEq<u64> for signal_schema::StateDigest {
    fn eq(&self, other: &u64) -> bool {
        self.payload() == other
    }
}

impl PartialOrd for signal_schema::StateDigest {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for signal_schema::StateDigest {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.payload().cmp(other.payload())
    }
}

impl std::ops::Deref for signal_schema::RecordsStashed {
    type Target = signal_schema::StashedObservation;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for signal_schema::RecordsObserved {
    type Target = signal_schema::ObservedRecords;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for signal_schema::RecordFound {
    type Target = signal_schema::FoundRecord;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for signal_schema::RecordsCounted {
    type Target = signal_schema::CountedRecords;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for signal_schema::SubscriptionStarted {
    type Target = signal_schema::IntentSubscription;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for signal_schema::ObservationTapped {
    type Target = signal_schema::ObserverSubscription;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for signal_schema::ObservationUntapped {
    type Target = signal_schema::ObserverRetraction;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for signal_schema::ObservedOperations {
    type Target = Vec<signal_schema::ObservedOperation>;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for signal_schema::CertaintyChanged {
    type Target = signal_schema::CertaintyChangeReceipt;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for signal_schema::RecordChanged {
    type Target = signal_schema::RecordChangeReceipt;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for crate::schema::sema::Recorded {
    type Target = SemaReceipt;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for crate::schema::sema::Removed {
    type Target = signal_schema::RemoveReceipt;

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

impl std::ops::Deref for signal_schema::RecordSet {
    type Target = Vec<signal_schema::ObservedRecord>;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl PartialEq<Vec<signal_schema::ObservedRecord>> for signal_schema::RecordSet {
    fn eq(&self, other: &Vec<signal_schema::ObservedRecord>) -> bool {
        self.payload() == other
    }
}

impl std::ops::Deref for nexus_schema::ReplyToSignal {
    type Target = Output;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for nexus_schema::CommandEffect {
    type Target = NexusEffectCommand;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for nexus_schema::SignalArrived {
    type Target = Input;

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

impl std::ops::Deref for signal_schema::Sent {
    type Target = SentMail;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl std::ops::Deref for signal_schema::Processed {
    type Target = ProcessedMail;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl SignalRejected {
    pub fn into_signal_output(self) -> signal_schema::signal::Signal<Output> {
        self.validation_error
            .into_signal_output()
            .with_origin_route(self.origin_route)
    }
}
