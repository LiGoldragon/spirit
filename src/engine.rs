use std::{convert::Infallible, sync::Mutex};

use crate::{
    DatabaseMarker, Entry, Input, InputNexus, Integer, MailIdentifier, MailLedgerEvent,
    MessageIdentifier, MessageProcessed, MessageProcessedHook, MessageSent, MessageSentHook,
    NexusEngine, NexusInput, NexusMail, NexusOutput, Output, ProcessedMail, Query, SemaEngine,
    SemaInput, SemaOutput, SentMail, ShortHeader, SignalRejection, ValidationError, store::Store,
};

#[derive(Debug, Default)]
pub struct Engine {
    signal_actor: SignalActor,
    store: Mutex<Store>,
    mail_ledger: MailLedger,
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
    pub fn handle(&self, input: Input) -> Output {
        let signal = match self.signal_actor.accept(input) {
            Ok(signal) => signal,
            Err(error) => return error.into_signal_output(self.database_marker()),
        };
        let identifier = signal.identifier();
        let nexus_step = signal
            .push_to_nexus(self, &mut self.mail_ledger.hook())
            .expect("spirit-next nexus is infallible");
        let sema_input = nexus_step.into_reply().into_sema_input();
        let sema_output = self.store.lock().expect("store lock").apply(sema_input);
        let output = NexusInput::Sema(sema_output)
            .into_nexus_output()
            .into_signal_output();
        let processed = MessageProcessed::new(identifier, output);
        processed
            .push_to(&mut self.mail_ledger.hook())
            .expect("spirit-next mail ledger is infallible");
        processed.into_reply()
    }

    pub fn record_count(&self) -> usize {
        self.store.lock().expect("store lock").len()
    }

    pub fn sent_message_count(&self) -> usize {
        self.mail_ledger.sent_message_count()
    }

    pub fn processed_message_count(&self) -> usize {
        self.mail_ledger.processed_message_count()
    }

    pub fn mail_ledger(&self) -> Vec<MailLedgerEvent> {
        self.mail_ledger.events()
    }

    pub fn database_marker(&self) -> DatabaseMarker {
        self.store.lock().expect("store lock").database_marker()
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

    pub fn push_to_nexus<Nexus, Hook, Error>(
        self,
        nexus: &Nexus,
        hook: &mut Hook,
    ) -> Result<MessageProcessed<Nexus::Reply>, Error>
    where
        Nexus: InputNexus<Error = Error>,
        Hook: MessageSentHook<Error = Error>,
    {
        let identifier = self.identifier();
        self.sent.push_to(hook)?;
        self.input.dispatch_mail_with_nexus(identifier, nexus)
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

impl InputNexus for Engine {
    type Reply = NexusOutput;
    type Error = Infallible;

    fn record(&self, mail: NexusMail<Entry>) -> Result<Self::Reply, Self::Error> {
        Ok(self.execute(mail.into_nexus_input()))
    }

    fn observe(&self, mail: NexusMail<Query>) -> Result<Self::Reply, Self::Error> {
        Ok(self.execute(mail.into_nexus_input()))
    }
}

impl NexusEngine for Engine {
    fn execute(&self, input: NexusInput) -> NexusOutput {
        input.into_nexus_output()
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
            short_header: ShortHeader(self.short_header),
        })
    }
}

impl MessageProcessed<Output> {
    pub fn processed_mail_event(&self) -> MailLedgerEvent {
        MailLedgerEvent::Processed(ProcessedMail {
            mail_identifier: MailIdentifier(self.identifier().as_integer()),
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
