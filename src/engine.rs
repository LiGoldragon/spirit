use std::{convert::Infallible, sync::Mutex};

use crate::{
    DatabaseMarker, Entry, Input, InputNexus, Integer, MailIdentifier, MailLedgerEvent,
    MessageIdentifier, MessageProcessed, MessageSent, NexusInput, NexusMail, NexusOutput, Output,
    ProcessedMail, Query, SemaInput, SemaOutput, SentMail, ShortHeader, store::Store,
};

#[derive(Debug, Default)]
pub struct Engine {
    store: Mutex<Store>,
    next_message_identifier: Mutex<Integer>,
    mail_ledger: Mutex<Vec<MailLedgerEvent>>,
}

impl Engine {
    pub fn handle(&self, input: Input) -> Output {
        let identifier = self.issue_message_identifier();
        self.remember_message_sent(input.message_sent(identifier));
        let nexus_step = input
            .dispatch_mail_with_nexus(identifier, self)
            .expect("spirit-next nexus is infallible");
        let sema_input = nexus_step.into_reply().into_sema_input();
        let sema_output = self.store.lock().expect("store lock").apply(sema_input);
        let output = NexusInput::Sema(sema_output)
            .into_nexus_output()
            .into_signal_output();
        let processed = MessageProcessed::new(identifier, output);
        self.remember_message_processed(&processed);
        processed.into_reply()
    }

    pub fn record_count(&self) -> usize {
        self.store.lock().expect("store lock").len()
    }

    pub fn sent_message_count(&self) -> usize {
        self.mail_ledger
            .lock()
            .expect("mail ledger lock")
            .iter()
            .filter(|event| event.is_sent())
            .count()
    }

    pub fn processed_message_count(&self) -> usize {
        self.mail_ledger
            .lock()
            .expect("mail ledger lock")
            .iter()
            .filter(|event| event.is_processed())
            .count()
    }

    pub fn mail_ledger(&self) -> Vec<MailLedgerEvent> {
        self.mail_ledger.lock().expect("mail ledger lock").clone()
    }

    fn issue_message_identifier(&self) -> MessageIdentifier {
        let mut next = self
            .next_message_identifier
            .lock()
            .expect("message identifier lock");
        *next += 1;
        MessageIdentifier(*next)
    }

    fn remember_message_sent(&self, event: MessageSent) {
        self.mail_ledger
            .lock()
            .expect("mail ledger lock")
            .push(event.into_mail_ledger_event());
    }

    fn remember_message_processed(&self, event: &MessageProcessed<Output>) {
        self.mail_ledger
            .lock()
            .expect("mail ledger lock")
            .push(event.processed_mail_event());
    }
}

impl InputNexus for Engine {
    type Reply = NexusOutput;
    type Error = Infallible;

    fn record(&self, mail: NexusMail<Entry>) -> Result<Self::Reply, Self::Error> {
        Ok(mail.into_nexus_input().into_nexus_output())
    }

    fn observe(&self, mail: NexusMail<Query>) -> Result<Self::Reply, Self::Error> {
        Ok(mail.into_nexus_input().into_nexus_output())
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
        }
    }
}
