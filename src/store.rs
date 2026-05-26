use crate::{Entry, ErrorMessage, Query, RecordIdentifier, RecordSet, SemaCommand, SemaResponse};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRecord {
    pub identifier: u64,
    pub entry: Entry,
}

#[derive(Clone, Debug)]
pub struct Store {
    next_identifier: u64,
    records: Vec<StoredRecord>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            next_identifier: 1,
            records: Vec::new(),
        }
    }
}

impl Store {
    pub fn apply(&mut self, command: SemaCommand) -> SemaResponse {
        match command {
            SemaCommand::Record(entry) => {
                let identifier = self.record(entry);
                SemaResponse::Recorded(RecordIdentifier(identifier))
            }
            SemaCommand::Observe(query) => match self.observe(&query) {
                Some(entry) => SemaResponse::Observed(RecordSet(entry)),
                None => SemaResponse::Missed(ErrorMessage(String::from("no matching record"))),
            },
        }
    }

    fn record(&mut self, entry: Entry) -> u64 {
        let identifier = self.next_identifier;
        self.next_identifier += 1;
        self.records.push(StoredRecord { identifier, entry });
        identifier
    }

    fn observe(&self, query: &Query) -> Option<Entry> {
        self.records
            .iter()
            .find(|record| record.entry.matches(query))
            .map(|record| record.entry.clone())
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl Entry {
    pub fn matches(&self, query: &Query) -> bool {
        self.topic == query.topic && self.kind == query.kind
    }
}
