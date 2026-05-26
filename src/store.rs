use crate::{Entry, Kind, Query, Topic};

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
    pub fn record(&mut self, entry: Entry) -> u64 {
        let identifier = self.next_identifier;
        self.next_identifier += 1;
        self.records.push(StoredRecord { identifier, entry });
        identifier
    }

    pub fn observe(&self, query: &Query) -> Option<Entry> {
        self.records
            .iter()
            .find(|record| entry_matches(&record.entry, &query.topic, &query.kind))
            .map(|record| record.entry.clone())
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

fn entry_matches(entry: &Entry, topic: &Topic, kind: &Kind) -> bool {
    entry.topic == *topic && entry.kind == *kind
}
