use std::{convert::Infallible, sync::Mutex as StdMutex};

use tokio::sync::Mutex;

use crate::{
    nexus::Nexus,
    schema::{
        meta_signal::{ConfigureReceipt, ConfigureRequest, Output as MetaOutput},
        nexus::{self as nexus_schema, NexusAction, NexusEngine, NexusWork},
        signal::{
            self as signal_schema, DatabaseMarker, EngineStartFailure, EngineStopFailure, Entry,
            ErrorReport, Input, Integer, IntentEvent, MailLedgerEvent, MessageIdentifier,
            MessageProcessed, MessageProcessedHook, MessageSent, MessageSentHook, OriginRoute,
            Output, ProcessedMail, Query, SemaReceipt, SentMail, SignalEngine, SignalRejection,
            TopicMatch, Topics, ValidationError,
        },
    },
    store::{Store, StoreError},
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, TraceEvent, TraceLog, schema::signal::SignalObjectName};

const ORIGIN_ROUTE_BASE: Integer = 1_000_000;

/// The daemon runtime: a thin composer of the three execution centers.
///
/// `Engine` owns the Signal admission actor and the Nexus mail keeper.
/// Nexus owns the durable SEMA store and the mail ledger. `Engine::handle`
/// runs the record-970 flow as a composition — it does NOT call the store
/// directly; the SEMA invocation lives inside Nexus, which holds the mail
/// in a being-processed state across it.
#[derive(Debug)]
pub struct Engine {
    signal_actor: SignalActor,
    nexus: Mutex<Nexus>,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
}

#[derive(Debug, Default)]
pub struct SignalActor {
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
                signal_actor: SignalActor::default(),
                nexus: Mutex::new(Nexus::new(store)),
            }
        }
    }

    #[cfg(feature = "testing-trace")]
    pub fn new_with_trace(store: Store, trace_log: TraceLog) -> Self {
        Self {
            signal_actor: SignalActor::with_trace(trace_log.clone()),
            nexus: Mutex::new(Nexus::new_with_trace(store, trace_log.clone())),
            trace_log,
        }
    }

    #[cfg(feature = "testing-trace")]
    pub fn trace_events(&self) -> Vec<TraceEvent> {
        self.trace_log.events()
    }

    pub fn start(&self) -> Result<(), EngineStartFailure> {
        {
            let mut nexus = self.nexus.try_lock().map_err(|_| {
                EngineStartFailure::ResourceBusy(String::from("nexus startup lock"))
            })?;
            NexusEngine::on_start(&mut *nexus)?;
        }
        self.signal_actor.start()
    }

    pub fn stop(&self) -> Result<(), EngineStopFailure> {
        self.signal_actor.stop()?;
        let mut nexus = self
            .nexus
            .try_lock()
            .map_err(|_| EngineStopFailure::ResourceLocked(String::from("nexus shutdown lock")))?;
        NexusEngine::on_stop(&mut *nexus)?;
        Ok(())
    }

    /// Run one request through Signal admission, the NexusEngine
    /// composition, and the durable SEMA store.
    ///
    /// Signal admits the input (mints the origin route, issues an
    /// identifier, and validates) before any deeper layer sees it. The
    /// sent hook fires at the Signal→Nexus handoff; the processed hook
    /// fires after the NexusEngine returns its reply.
    pub fn handle(&self, input: Input) -> signal_schema::signal::Signal<Output> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("spirit sync handle runtime")
            .block_on(self.handle_async(input))
    }

    pub async fn handle_async(&self, input: Input) -> signal_schema::signal::Signal<Output> {
        let accepted = match self.signal_actor.admit(input) {
            Ok(accepted) => accepted,
            Err(rejected) => {
                let output = rejected.into_signal_output(self.database_marker_async().await);
                #[cfg(feature = "testing-trace")]
                self.signal_actor.trace_signal_rejected();
                #[cfg(feature = "testing-trace")]
                self.signal_actor.trace_signal_replied();
                return output;
            }
        };
        let mut nexus = self.nexus.lock().await;
        accepted.process_with(&self.signal_actor, &mut nexus).await
    }

    pub fn record_count(&self) -> usize {
        self.nexus.blocking_lock().store().len()
    }

    pub fn sent_message_count(&self) -> usize {
        self.nexus
            .blocking_lock()
            .mail_ledger()
            .sent_message_count()
    }

    pub fn processed_message_count(&self) -> usize {
        self.nexus
            .blocking_lock()
            .mail_ledger()
            .processed_message_count()
    }

    pub fn mail_ledger(&self) -> Vec<MailLedgerEvent> {
        self.nexus.blocking_lock().mail_ledger().events()
    }

    pub fn database_marker(&self) -> DatabaseMarker {
        self.nexus.blocking_lock().database_marker()
    }

    pub async fn database_marker_async(&self) -> DatabaseMarker {
        self.nexus.lock().await.database_marker()
    }

    /// Apply an owner-only meta `Configure` request: store WHERE the SEPARATE
    /// archive database lives, and reply with the now-active target plus the
    /// live database marker.
    ///
    /// This is the owner-config meta-socket effect. It records the archive
    /// target the peer-callable `CollectRemovalCandidates` will write to; it
    /// does NOT open, move, or touch the live database, and it never re-enters
    /// the Signal -> Nexus -> SEMA working pipeline (there is no SEMA log
    /// write). It locks the same single-flight Nexus mutex the working path
    /// uses, so a reconfigure and a working write can never run concurrently.
    /// Storing the target is infallible — the archive database is opened lazily
    /// later, not here — so this always replies `Configured`. The
    /// `ConfigureRejection` / `ArchiveTargetUnwritable` arm of the contract is
    /// reserved for a future eager-validation policy.
    pub fn configure(&self, request: ConfigureRequest) -> MetaOutput {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("spirit sync configure runtime")
            .block_on(self.configure_async(request))
    }

    pub async fn configure_async(&self, request: ConfigureRequest) -> MetaOutput {
        let archive_database_target = request.into_payload();
        let mut nexus = self.nexus.lock().await;
        nexus.set_archive_target(archive_database_target.clone());
        MetaOutput::configured(ConfigureReceipt {
            archive_database_target,
            database_marker: nexus.database_marker(),
        })
    }

    pub fn intent_recorded_event(
        &self,
        receipt: &SemaReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("spirit sync intent event runtime")
            .block_on(self.intent_recorded_event_async(receipt))
    }

    pub async fn intent_recorded_event_async(
        &self,
        receipt: &SemaReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        self.nexus.lock().await.intent_recorded_event(receipt)
    }
}

impl SignalActor {
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
        MessageIdentifier(*next)
    }

    fn issue_origin_route(&self) -> OriginRoute {
        let mut next = self.next_origin_route.lock().expect("origin route lock");
        *next += 1;
        OriginRoute(ORIGIN_ROUTE_BASE + *next)
    }
}

impl SignalEngine for SignalActor {
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
        let nexus_output = NexusEngine::execute(nexus, nexus_input).await;
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
            Self::State(statement) => statement.validate(),
            Self::Record(record) => record.validate(),
            Self::Observe(observe) => observe.validate(),
            Self::Lookup(_)
            | Self::Remove(_)
            | Self::ChangeCertainty(_)
            | Self::LookupStash(_)
            | Self::Tap(_)
            | Self::Untap(_) => Ok(()),
            Self::CollectRemovalCandidates(collection) => collection.payload().validate(),
            Self::SubscribeIntent(query) => query.validate(),
            Self::Count(count) => count.validate(),
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
        if self.topics.is_empty() {
            return Err(ValidationError::EmptyTopic);
        }
        if self.topics.iter().any(|topic| topic.trim().is_empty()) {
            return Err(ValidationError::EmptyTopic);
        }
        if self.description.trim().is_empty() {
            return Err(ValidationError::EmptyDescription);
        }
        Ok(())
    }
}

impl Query {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.topic_match.validate()
    }
}

impl TopicMatch {
    pub fn validate(&self) -> Result<(), ValidationError> {
        let topics = self.topics();
        if topics.is_empty() {
            return Err(ValidationError::EmptyQueryTopic);
        }
        if topics.iter().any(|topic| topic.trim().is_empty()) {
            return Err(ValidationError::EmptyQueryTopic);
        }
        Ok(())
    }

    pub fn topics(&self) -> &Topics {
        match self {
            Self::Partial(partial) => partial,
            Self::Full(full) => full,
        }
    }

    pub fn matches(&self, entry_topics: &Topics) -> bool {
        match self {
            Self::Partial(partial) => partial
                .iter()
                .any(|topic| entry_topics.iter().any(|entry_topic| entry_topic == topic)),
            Self::Full(full) => full
                .iter()
                .all(|topic| entry_topics.iter().any(|entry_topic| entry_topic == topic)),
        }
    }
}

impl MessageIdentifier {
    pub fn as_integer(&self) -> Integer {
        self.0
    }
}

impl MessageSent {
    pub fn into_mail_ledger_event(self) -> MailLedgerEvent {
        MailLedgerEvent::sent(SentMail {
            mail_identifier: self.identifier.as_integer(),
            origin_route: self.origin_route(),
            short_header: self.short_header,
        })
    }
}

impl MessageProcessed<Output> {
    pub fn processed_mail_event(&self) -> MailLedgerEvent {
        MailLedgerEvent::processed(ProcessedMail {
            mail_identifier: self.identifier().as_integer(),
            origin_route: self.origin_route(),
            database_marker: self.reply.database_marker(),
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

impl Output {
    pub fn database_marker(&self) -> DatabaseMarker {
        match self {
            Self::RecordAccepted(receipt) => receipt.database_marker.clone(),
            Self::RecordsObserved(records) => records.database_marker.clone(),
            Self::RecordsStashed(stashed) => stashed.database_marker.clone(),
            Self::RecordFound(record) => record.database_marker.clone(),
            Self::RecordsCounted(records) => records.database_marker.clone(),
            Self::RecordRemoved(receipt) => receipt.database_marker.clone(),
            Self::CertaintyChanged(receipt) => receipt.database_marker.clone(),
            Self::RemovalCandidatesCollected(collection) => collection.database_marker.clone(),
            Self::ObservationTapped(subscription) => subscription.database_marker.clone(),
            Self::ObservationUntapped(retraction) => retraction.database_marker.clone(),
            Self::SubscriptionStarted(subscription) => subscription.database_marker.clone(),
            Self::Event(event) => event.database_marker(),
            Self::Error(report) => report.database_marker.clone(),
            Self::Rejected(rejection) => rejection.database_marker.clone(),
        }
    }
}

impl crate::schema::signal::IntentEvent {
    pub fn database_marker(&self) -> DatabaseMarker {
        match self {
            Self::IntentRecorded(recorded) => recorded.sema_receipt.database_marker.clone(),
        }
    }
}

impl DatabaseMarker {
    pub fn zero() -> Self {
        Self {
            commit_sequence: 0,
            state_digest: 0,
        }
    }
}

impl ValidationError {
    pub fn into_signal_output(self, database_marker: DatabaseMarker) -> Output {
        Output::rejected(SignalRejection {
            validation_error: self,
            database_marker,
        })
    }
}

impl nexus_schema::nexus::Nexus<NexusAction> {
    pub fn into_signal_output(self) -> signal_schema::signal::Signal<Output> {
        let origin_route = self.origin_route();
        match self.into_root() {
            NexusAction::ReplyToSignal(output) => output.with_origin_route(origin_route.into()),
            _ => Output::error(ErrorReport {
                error_message: String::from("nexus returned non-signal action"),
                database_marker: DatabaseMarker::zero(),
            })
            .with_origin_route(origin_route.into()),
        }
    }
}

impl SignalRejected {
    pub fn into_signal_output(
        self,
        database_marker: DatabaseMarker,
    ) -> signal_schema::signal::Signal<Output> {
        self.validation_error
            .into_signal_output(database_marker)
            .with_origin_route(self.origin_route)
    }
}
