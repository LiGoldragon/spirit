use std::{convert::Infallible, sync::Mutex};

use crate::{
    DatabaseMarker, Entry, Input, Integer, MailIdentifier, MailLedgerEvent, MessageIdentifier,
    MessageProcessed, MessageProcessedHook, MessageSent, MessageSentHook, NexusMail, OriginRoute,
    Output, ProcessedMail, Query, RecordIdentifier, SentMail, ShortHeader, SignalRejection,
    TopicMatch, Topics, ValidationError,
    nexus::{BeingProcessed, FromMail, Mail, Nexus},
    schema::lib::signal as signal_plane,
    store::Store,
};

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
}

#[derive(Debug, Default)]
pub struct SignalActor {
    next_message_identifier: Mutex<Integer>,
    next_origin_route: Mutex<Integer>,
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
        Self {
            signal_actor: SignalActor::default(),
            nexus: Mutex::new(Nexus::new(store)),
        }
    }

    /// Run one request through Signal admission, Nexus mail-keeping, and
    /// the durable SEMA store.
    ///
    /// Signal validates and issues identity; the sent hook fires at the
    /// Signal→Nexus handoff; Nexus then HOLDS the mail in a
    /// being-processed state across the SEMA call and emits the processed
    /// event before the Signal output leaves.
    pub fn handle(&self, input: Input) -> signal_plane::Signal<Output> {
        let accepted = match self.signal_actor.accept(input) {
            Ok(accepted) => accepted,
            Err(rejected) => return rejected.into_signal_output(self.database_marker()),
        };
        let mut nexus = self.nexus.lock().expect("nexus lock");
        accepted.process_with(&mut nexus)
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
    pub fn accept(&self, input: Input) -> Result<SignalAccepted, SignalRejected> {
        let identifier = self.issue_message_identifier();
        let origin_route = self.issue_origin_route();
        input
            .validate()
            .map_err(|validation_error| SignalRejected {
                origin_route,
                validation_error,
            })?;
        let input = input.with_origin_route(origin_route);
        Ok(SignalAccepted {
            sent: input.message_sent(identifier),
            input,
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

impl SignalAccepted {
    pub fn identifier(&self) -> MessageIdentifier {
        self.sent.identifier
    }

    pub fn message_sent(&self) -> &MessageSent {
        &self.sent
    }

    /// Hand the validated mail to Nexus: fire the sent hook at the
    /// handoff, then let Nexus hold the mail across the SEMA call.
    ///
    /// The sent hook (the Signal→Nexus on_sent event) fires BEFORE the
    /// mail enters Nexus, so an observer sees the handoff before any SEMA
    /// state changes. The mail is then owned by Nexus in a being-processed
    /// type-state until the SEMA reply turns it into the Signal output.
    pub fn process_with(self, nexus: &mut Nexus) -> signal_plane::Signal<Output> {
        self.sent
            .push_to(&mut nexus.mail_ledger().hook())
            .expect("spirit-next mail ledger is infallible");
        let identifier = self.identifier();
        let origin_route = self.input.origin_route();
        match self.input.into_root() {
            Input::Record(entry) => nexus.process(NexusMail::new(identifier, origin_route, entry)),
            Input::Observe(query) => nexus.process(NexusMail::new(identifier, origin_route, query)),
            Input::Remove(record_identifier) => {
                nexus.process(NexusMail::new(identifier, origin_route, record_identifier))
            }
        }
    }

    /// Lower the accepted mail to its in-flight Nexus phase without
    /// running SEMA — the witness that Nexus owns the mail in a
    /// being-processed type-state. Used by tests to observe the held mail.
    pub fn into_being_processed(self) -> Mail<BeingProcessed>
    where
        Mail<BeingProcessed>: FromMail<Entry> + FromMail<Query> + FromMail<RecordIdentifier>,
    {
        let identifier = self.identifier();
        let origin_route = self.input.origin_route();
        match self.input.into_root() {
            Input::Record(entry) => {
                Mail::<BeingProcessed>::from_mail(NexusMail::new(identifier, origin_route, entry))
            }
            Input::Observe(query) => {
                Mail::<BeingProcessed>::from_mail(NexusMail::new(identifier, origin_route, query))
            }
            Input::Remove(record_identifier) => Mail::<BeingProcessed>::from_mail(NexusMail::new(
                identifier,
                origin_route,
                record_identifier,
            )),
        }
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
            Self::Remove(_) => Ok(()),
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
