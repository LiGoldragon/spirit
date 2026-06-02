use std::{convert::Infallible, sync::Mutex};

use crate::{
    DatabaseMarker, Entry, Input, Integer, MailIdentifier, MailLedgerEvent, MessageIdentifier,
    MessageProcessed, MessageProcessedHook, MessageSent, MessageSentHook, NexusAction, NexusEngine,
    NexusWork, OriginRoute, Output, ProcessedMail, Query, SentMail, ShortHeader, SignalEngine,
    SignalRejection, TopicMatch, Topics, ValidationError,
    nexus::Nexus,
    schema::lib::{nexus as nexus_plane, signal as signal_plane},
    store::Store,
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, SignalObjectName, TraceEvent, TraceLog};

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
    next_message_identifier: Mutex<Integer>,
    next_origin_route: Mutex<Integer>,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
}

#[derive(Debug)]
pub struct SignalAccepted {
    input: signal_plane::Signal<Input>,
    sent: MessageSent,
}

#[derive(Debug)]
pub struct SignalRejected {
    origin_route: OriginRoute,
    validation_error: ValidationError,
}

#[derive(Debug, Default)]
pub struct MailLedger {
    events: Mutex<Vec<MailLedgerEvent>>,
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
    pub fn trace_events(&self) -> Vec<crate::TraceEvent> {
        self.trace_log.events()
    }

    /// Run one request through Signal admission, the NexusEngine
    /// composition, and the durable SEMA store.
    ///
    /// Signal admits the input (mints the origin route, issues an
    /// identifier, and validates) before any deeper layer sees it. The
    /// sent hook fires at the Signal→Nexus handoff; the processed hook
    /// fires after the NexusEngine returns its reply.
    pub fn handle(&self, input: Input) -> signal_plane::Signal<Output> {
        let accepted = match self.signal_actor.admit(input) {
            Ok(accepted) => accepted,
            Err(rejected) => {
                let output = rejected.into_signal_output(self.database_marker());
                #[cfg(feature = "testing-trace")]
                self.signal_actor.trace_signal_rejected();
                #[cfg(feature = "testing-trace")]
                self.signal_actor.trace_signal_replied();
                return output;
            }
        };
        let mut nexus = self.nexus.lock().expect("nexus lock");
        accepted.process_with(&self.signal_actor, &mut nexus)
    }

    pub fn record_count(&self) -> usize {
        self.nexus.lock().expect("nexus lock").store().len()
    }

    pub fn sent_message_count(&self) -> usize {
        self.nexus
            .lock()
            .expect("nexus lock")
            .mail_ledger()
            .sent_message_count()
    }

    pub fn processed_message_count(&self) -> usize {
        self.nexus
            .lock()
            .expect("nexus lock")
            .mail_ledger()
            .processed_message_count()
    }

    pub fn mail_ledger(&self) -> Vec<MailLedgerEvent> {
        self.nexus
            .lock()
            .expect("nexus lock")
            .mail_ledger()
            .events()
    }

    pub fn database_marker(&self) -> DatabaseMarker {
        self.nexus.lock().expect("nexus lock").database_marker()
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
    #[cfg(feature = "testing-trace")]
    fn trace_signal_activation(&self, object_name: SignalObjectName) {
        self.trace_log
            .record(TraceEvent::new(ObjectName::Signal(object_name)));
    }

    fn triage_inner(&self, input: signal_plane::Signal<Input>) -> nexus_plane::Nexus<NexusWork> {
        let origin_route = input.origin_route();
        NexusWork::from(input.into_root()).with_origin_route(origin_route)
    }

    fn reply_inner(&self, output: nexus_plane::Nexus<NexusAction>) -> signal_plane::Signal<Output> {
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
    pub fn process_with<Signal>(
        self,
        signal_engine: &Signal,
        nexus: &mut Nexus,
    ) -> signal_plane::Signal<Output>
    where
        Signal: SignalEngine,
    {
        self.sent
            .push_to(&mut nexus.mail_ledger().hook())
            .expect("spirit-next mail ledger is infallible");
        let identifier = self.identifier();
        let nexus_input = signal_engine.triage(self.input);
        let origin_route = nexus_input.origin_route();
        let nexus_output = NexusEngine::execute(nexus, nexus_input);
        let signal_output = signal_engine.reply(nexus_output);
        MessageProcessed::new(identifier, origin_route, signal_output.root().clone())
            .push_to(&mut nexus.mail_ledger().hook())
            .expect("spirit-next mail ledger is infallible");
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
            Self::Record(entry) => entry.validate(),
            Self::Observe(query) => query.validate(),
            Self::Lookup(_) | Self::Remove(_) | Self::LookupStash(_) => Ok(()),
            Self::Count(query) => query.validate(),
        }
    }
}

impl Entry {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.topics.validate()?;
        if self.description.0.trim().is_empty() {
            return Err(ValidationError::EmptyDescription);
        }
        Ok(())
    }
}

impl Topics {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.0.is_empty() {
            return Err(ValidationError::EmptyTopic);
        }
        if self.0.iter().any(|topic| topic.0.trim().is_empty()) {
            return Err(ValidationError::EmptyTopic);
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
        self.topics()
            .validate()
            .map_err(|_| ValidationError::EmptyQueryTopic)
    }

    pub fn topics(&self) -> &Topics {
        match self {
            Self::Partial(topics) | Self::Full(topics) => topics,
        }
    }

    pub fn matches(&self, entry_topics: &Topics) -> bool {
        match self {
            Self::Partial(topics) => topics.0.iter().any(|topic| {
                entry_topics
                    .0
                    .iter()
                    .any(|entry_topic| entry_topic == topic)
            }),
            Self::Full(topics) => topics.0.iter().all(|topic| {
                entry_topics
                    .0
                    .iter()
                    .any(|entry_topic| entry_topic == topic)
            }),
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
        MailLedgerEvent::Sent(SentMail {
            mail_identifier: MailIdentifier(self.identifier.as_integer()),
            origin_route: self.origin_route(),
            short_header: ShortHeader(self.short_header),
        })
    }
}

impl MessageProcessed<Output> {
    pub fn processed_mail_event(&self) -> MailLedgerEvent {
        MailLedgerEvent::Processed(ProcessedMail {
            mail_identifier: MailIdentifier(self.identifier().as_integer()),
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
            Self::Error(report) => report.database_marker.clone(),
            Self::Rejected(rejection) => rejection.database_marker.clone(),
        }
    }
}

impl ValidationError {
    pub fn into_signal_output(self, database_marker: DatabaseMarker) -> Output {
        Output::Rejected(SignalRejection {
            validation_error: self,
            database_marker,
        })
    }
}

impl SignalRejected {
    pub fn into_signal_output(
        self,
        database_marker: DatabaseMarker,
    ) -> signal_plane::Signal<Output> {
        self.validation_error
            .into_signal_output(database_marker)
            .with_origin_route(self.origin_route)
    }
}
