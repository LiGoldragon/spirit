use std::sync::Mutex;

use crate::{Input, Output, SemaCommand, SemaResponse, store::Store};

#[derive(Debug, Default)]
pub struct Engine {
    store: Mutex<Store>,
}

impl Engine {
    pub fn handle(&self, input: Input) -> Output {
        let command = input.lower_to_sema();
        let response = self.store.lock().expect("store lock").apply(command);
        response.into_output()
    }

    pub fn record_count(&self) -> usize {
        self.store.lock().expect("store lock").len()
    }
}

impl Input {
    pub fn lower_to_sema(self) -> SemaCommand {
        match self {
            Self::Record(entry) => SemaCommand::Record(entry),
            Self::Observe(query) => SemaCommand::Observe(query),
        }
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
