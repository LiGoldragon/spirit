use std::{convert::Infallible, sync::Mutex as StdMutex};

use crate::{
    nexus::Nexus,
    schema::{
        meta_signal::{ConfigureReceipt, ConfigureRequest, Output as MetaOutput},
        nexus::{self as nexus_schema, NexusAction, NexusEffectCommand, NexusEngine, NexusWork},
        sema::ErrorReport,
        signal::{
            self as signal_schema, CertaintySelection, DatabaseMarker, Description,
            EngineStartFailure, EngineStopFailure, Entry, ErrorMessage, Input, Integer,
            IntentEvent, MailIdentifier, MailLedgerEvent, MessageIdentifier, MessageProcessed,
            MessageProcessedHook, MessageSent, MessageSentHook, OriginRoute, Output, Privacy,
            PrivacySelection, ProcessedMail, Query, RecordSelection, SemaReceipt, SentMail,
            ShortHeader, SignalEngine, SignalRejection, StatementText, Topic, TopicMatch, Topics,
            ValidationError,
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
            }
        }
    }

    #[cfg(feature = "testing-trace")]
    pub fn new_with_trace(store: Store, trace_log: TraceLog) -> Self {
        Self {
            signal_admission: SignalAdmission::with_trace(trace_log.clone()),
            nexus: Nexus::new_with_trace(store, trace_log.clone()),
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
                let output = rejected.into_signal_output(self.nexus.database_marker());
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
        let archive_database_target = request.into_payload();
        self.nexus
            .set_archive_target(archive_database_target.clone());
        MetaOutput::configured(ConfigureReceipt {
            archive_database_target,
            database_marker: self.nexus.database_marker(),
        })
    }

    pub async fn configure_async(&mut self, request: ConfigureRequest) -> MetaOutput {
        self.configure(request)
    }

    pub fn intent_recorded_event(
        &self,
        receipt: &SemaReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        self.nexus.intent_recorded_event(receipt)
    }

    pub async fn intent_recorded_event_async(
        &self,
        receipt: &SemaReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        self.intent_recorded_event(receipt)
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
            Self::Observe(observe) => observe.payload().validate(),
            Self::PublicRecords(selection) => selection.payload().validate(),
            Self::PrivateRecords(selection) => selection.payload().validate(),
            Self::Lookup(_)
            | Self::Remove(_)
            | Self::ChangeCertainty(_)
            | Self::LookupStash(_)
            | Self::Tap(_)
            | Self::Untap(_)
            | Self::Version => Ok(()),
            Self::ChangeRecord(change) => change.payload().validate(),
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

impl crate::schema::signal::RecordChange {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.entry.validate()
    }
}

impl Query {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.topic_match.validate()
    }
}

impl crate::schema::signal::RemovalCandidateCollection {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.payload().validate()
    }
}

impl crate::schema::signal::RecordQuery {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.payload().validate()
    }
}

impl RecordSelection {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.topic_match.validate()
    }

    pub fn into_public_query(self) -> Query {
        Query {
            topic_match: self.topic_match,
            kind: self.kind,
            privacy_selection: PrivacySelection::default_observation_privacy(),
            certainty_selection: CertaintySelection::default_observation_certainty(),
        }
    }

    pub fn into_private_query(self) -> Query {
        Query {
            topic_match: self.topic_match,
            kind: self.kind,
            privacy_selection: PrivacySelection::at_least(Privacy::new(
                PrivacySelection::private_floor(),
            )),
            certainty_selection: CertaintySelection::default_observation_certainty(),
        }
    }
}

impl TopicMatch {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Any => Ok(()),
            Self::Partial(topics) => {
                if topics.payload().is_empty() {
                    return Err(ValidationError::EmptyQueryTopic);
                }
                if topics.payload().iter().any(|topic| topic.trim().is_empty()) {
                    return Err(ValidationError::EmptyQueryTopic);
                }
                Ok(())
            }
            Self::Full(topics) => {
                if topics.payload().is_empty() {
                    return Err(ValidationError::EmptyQueryTopic);
                }
                if topics.payload().iter().any(|topic| topic.trim().is_empty()) {
                    return Err(ValidationError::EmptyQueryTopic);
                }
                Ok(())
            }
        }
    }

    pub fn matches(&self, entry_topics: &Topics) -> bool {
        match self {
            Self::Any => true,
            Self::Partial(partial) => partial
                .payload()
                .iter()
                .any(|topic| entry_topics.iter().any(|entry_topic| entry_topic == topic)),
            Self::Full(full) => full
                .payload()
                .iter()
                .all(|topic| entry_topics.iter().any(|entry_topic| entry_topic == topic)),
        }
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
            Self::RecordAccepted(receipt) => receipt.payload().database_marker.clone(),
            Self::RecordsObserved(records) => records.payload().database_marker.clone(),
            Self::RecordsStashed(stashed) => stashed.payload().database_marker.clone(),
            Self::RecordFound(record) => record.payload().database_marker.clone(),
            Self::RecordsCounted(records) => records.payload().database_marker.clone(),
            Self::RecordRemoved(receipt) => receipt.payload().database_marker.clone(),
            Self::CertaintyChanged(receipt) => receipt.payload().database_marker.clone(),
            Self::RecordChanged(receipt) => receipt.payload().database_marker.clone(),
            Self::RemovalCandidatesCollected(collection) => {
                collection.payload().database_marker.clone()
            }
            Self::ObservationTapped(subscription) => subscription.payload().database_marker.clone(),
            Self::ObservationUntapped(retraction) => retraction.payload().database_marker.clone(),
            Self::SubscriptionStarted(subscription) => {
                subscription.payload().database_marker.clone()
            }
            Self::VersionReported(report) => report.payload().database_marker.clone(),
            Self::Event(event) => event.database_marker(),
            Self::Error(report) => report.payload().database_marker.clone(),
            Self::Rejected(rejection) => rejection.payload().database_marker.clone(),
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
            commit_sequence: signal_schema::CommitSequence::new(0),
            state_digest: signal_schema::StateDigest::new(0),
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
            NexusAction::ReplyToSignal(output) => {
                output.into_payload().with_origin_route(origin_route.into())
            }
            _ => Output::error(ErrorReport {
                error_message: ErrorMessage::new("nexus returned non-signal action"),
                database_marker: DatabaseMarker::zero(),
            })
            .with_origin_route(origin_route.into()),
        }
    }
}

impl Topic {
    pub fn as_str(&self) -> &str {
        self.payload()
    }

    pub fn trim(&self) -> &str {
        self.as_str().trim()
    }
}

impl Topics {
    pub fn from_strings(topics: Vec<String>) -> Self {
        Self::new(topics.into_iter().map(Topic::new).collect())
    }

    pub fn is_empty(&self) -> bool {
        self.payload().is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Topic> {
        self.payload().iter()
    }
}

impl PartialEq<Vec<String>> for Topics {
    fn eq(&self, other: &Vec<String>) -> bool {
        self.payload().iter().map(Topic::payload).eq(other.iter())
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
    pub fn weight(&self) -> u64 {
        self.payload().weight()
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

impl std::ops::Deref for signal_schema::RecordAccepted {
    type Target = SemaReceipt;

    fn deref(&self) -> &Self::Target {
        self.payload()
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
    type Target = Vec<Entry>;

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl PartialEq<Vec<Entry>> for signal_schema::RecordSet {
    fn eq(&self, other: &Vec<Entry>) -> bool {
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
    pub fn into_signal_output(
        self,
        database_marker: DatabaseMarker,
    ) -> signal_schema::signal::Signal<Output> {
        self.validation_error
            .into_signal_output(database_marker)
            .with_origin_route(self.origin_route)
    }
}
