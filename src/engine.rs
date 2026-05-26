use std::sync::Mutex;

use crate::{ErrorMessage, Input, Output, RecordIdentifier, RecordSet, store::Store};

#[derive(Debug, Default)]
pub struct Engine {
    store: Mutex<Store>,
}

impl Engine {
    pub fn handle(&self, input: Input) -> Output {
        match input {
            Input::Record(entry) => {
                let identifier = self.store.lock().expect("store lock").record(entry);
                Output::RecordAccepted(RecordIdentifier(identifier))
            }
            Input::Observe(query) => match self.store.lock().expect("store lock").observe(&query) {
                Some(entry) => Output::RecordsObserved(RecordSet(entry)),
                None => Output::Error(ErrorMessage(String::from("no matching record"))),
            },
        }
    }

    pub fn record_count(&self) -> usize {
        self.store.lock().expect("store lock").len()
    }
}
