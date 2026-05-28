use std::{convert::Infallible, sync::Mutex};

use crate::{
    DatabaseMarker, Entry, Input, Integer, MailIdentifier, MailLedgerEvent, MessageIdentifier,
    MessageProcessed, MessageProcessedHook, MessageSent, MessageSentHook, NexusInput, NexusMail,
    NexusOutput, Output, ProcessedMail, Query, SemaInput, SemaOutput, SentMail, ShortHeader,
    SignalRejection, ValidationError,
    nexus::{BeingProcessed, FromMail, Mail, Nexus},
    store::Store,
};

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
}

#[derive(Debug)]
pub struct SignalAccepted {
    input: Input,
    sent: MessageSent,
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
    pub fn handle(&self, input: Input) -> Output {
        let accepted = match self.signal_actor.accept(input) {
            Ok(accepted) => accepted,
            Err(error) => return error.into_signal_output(self.database_marker()),
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
    pub fn accept(&self, input: Input) -> Result<SignalAccepted, ValidationError> {
        input.validate()?;
        let identifier = self.issue_message_identifier();
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
    pub fn process_with(self, nexus: &mut Nexus) -> Output {
        self.sent
            .push_to(&mut nexus.mail_ledger().hook())
            .expect("spirit-next mail ledger is infallible");
        let identifier = self.identifier();
        match self.input {
            Input::Record(entry) => nexus.process(NexusMail::new(identifier, entry)),
            Input::Observe(query) => nexus.process(NexusMail::new(identifier, query)),
        }
    }

    /// Lower the accepted mail to its in-flight Nexus phase without
    /// running SEMA — the witness that Nexus owns the mail in a
    /// being-processed type-state. Used by tests to observe the held mail.
    pub fn into_being_processed(self) -> Mail<BeingProcessed>
    where
        Mail<BeingProcessed>: FromMail<Entry> + FromMail<Query>,
    {
        let identifier = self.identifier();
        match self.input {
            Input::Record(entry) => {
                Mail::<BeingProcessed>::from_mail(NexusMail::new(identifier, entry))
            }
            Input::Observe(query) => {
                Mail::<BeingProcessed>::from_mail(NexusMail::new(identifier, query))
            }
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
        }
    }
}

impl Entry {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.topic.0.trim().is_empty() {
            return Err(ValidationError::EmptyTopic);
        }
        if self.description.0.trim().is_empty() {
            return Err(ValidationError::EmptyDescription);
        }
        Ok(())
    }
}

impl Query {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.topic.0.trim().is_empty() {
            return Err(ValidationError::EmptyQueryTopic);
        }
        Ok(())
    }
}

impl NexusMail<Entry> {
    pub fn into_nexus_input(self) -> NexusInput {
        NexusInput::Signal(Input::Record(self.into_payload()))
    }
}

impl NexusMail<Query> {
    pub fn into_nexus_input(self) -> NexusInput {
        NexusInput::Signal(Input::Observe(self.into_payload()))
    }
}

impl NexusInput {
    pub fn into_nexus_output(self) -> NexusOutput {
        match self {
            Self::Signal(Input::Record(entry)) => NexusOutput::Sema(SemaInput::Record(entry)),
            Self::Signal(Input::Observe(query)) => NexusOutput::Sema(SemaInput::Observe(query)),
            Self::Sema(output) => NexusOutput::Signal(output.into_signal_output()),
        }
    }
}

impl NexusOutput {
    pub fn into_sema_input(self) -> SemaInput {
        match self {
            Self::Sema(input) => input,
            Self::Signal(_) => panic!("nexus output is a signal reply, not a SEMA input"),
        }
    }

    pub fn into_signal_output(self) -> Output {
        match self {
            Self::Signal(output) => output,
            Self::Sema(_) => panic!("nexus output is a SEMA input, not a signal reply"),
        }
    }
}

impl SemaOutput {
    pub fn into_signal_output(self) -> Output {
        match self {
            Self::Recorded(identifier) => Output::RecordAccepted(identifier),
            Self::Observed(records) => Output::RecordsObserved(records),
            Self::Missed(error) => Output::Error(error),
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
