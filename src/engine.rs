use std::{convert::Infallible, sync::Mutex};

use crate::{
    Entry, Input, InputNexus, MessageIdentifier, MessageProcessed, MessageSent, NexusMail, Output,
    Query, SemaCommand, SemaResponse, store::Store,
};

#[derive(Debug, Default)]
pub struct Engine {
    store: Mutex<Store>,
    next_message_identifier: Mutex<u128>,
    sent_messages: Mutex<Vec<MessageSent>>,
    processed_messages: Mutex<Vec<MessageIdentifier>>,
}

impl Engine {
    pub fn handle(&self, input: Input) -> Output {
        let identifier = self.issue_message_identifier();
        self.remember_message_sent(input.message_sent(identifier));
        let processed = input
            .dispatch_mail_with_nexus(identifier, self)
            .expect("spirit-next nexus is infallible");
        self.remember_message_processed(&processed);
        processed.into_reply()
    }

    pub fn record_count(&self) -> usize {
        self.store.lock().expect("store lock").len()
    }

    pub fn sent_message_count(&self) -> usize {
        self.sent_messages.lock().expect("sent messages lock").len()
    }

    pub fn processed_message_count(&self) -> usize {
        self.processed_messages
            .lock()
            .expect("processed messages lock")
            .len()
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
        self.sent_messages
            .lock()
            .expect("sent messages lock")
            .push(event);
    }

    fn remember_message_processed(&self, event: &MessageProcessed<Output>) {
        self.processed_messages
            .lock()
            .expect("processed messages lock")
            .push(event.identifier());
    }

    fn apply_sema_command(&self, command: SemaCommand) -> Output {
        let response = self.store.lock().expect("store lock").apply(command);
        response.into_output()
    }
}

impl InputNexus for Engine {
    type Reply = Output;
    type Error = Infallible;

    fn record(&self, mail: NexusMail<Entry>) -> Result<Self::Reply, Self::Error> {
        Ok(self.apply_sema_command(mail.into_sema_command()))
    }

    fn observe(&self, mail: NexusMail<Query>) -> Result<Self::Reply, Self::Error> {
        Ok(self.apply_sema_command(mail.into_sema_command()))
    }
}

impl NexusMail<Entry> {
    pub fn into_sema_command(self) -> SemaCommand {
        SemaCommand::Record(self.into_payload())
    }
}

impl NexusMail<Query> {
    pub fn into_sema_command(self) -> SemaCommand {
        SemaCommand::Observe(self.into_payload())
    }
}

impl SemaResponse {
    pub fn into_output(self) -> Output {
        match self {
            Self::Recorded(identifier) => Output::RecordAccepted(identifier),
            Self::Observed(records) => Output::RecordsObserved(records),
            Self::Missed(error) => Output::Error(error),
        }
    }
}
