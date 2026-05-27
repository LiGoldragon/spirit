use crate::{
    CommitSequence, DatabaseMarker, Entry, ErrorMessage, ErrorReport, Magnitude, ObservedRecords,
    Query, RecordIdentifier, RecordSet, SemaCommand, SemaReceipt, SemaResponse, StateDigest,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRecord {
    pub identifier: u64,
    pub entry: Entry,
}

#[derive(Clone, Debug)]
pub struct Store {
    next_identifier: u64,
    commit_sequence: u64,
    records: Vec<StoredRecord>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            next_identifier: 1,
            commit_sequence: 0,
            records: Vec::new(),
        }
    }
}

impl Store {
    pub fn apply(&mut self, command: SemaCommand) -> SemaResponse {
        match command {
            SemaCommand::Record(entry) => {
                let identifier = self.record(entry);
                SemaResponse::Recorded(SemaReceipt {
                    record_identifier: RecordIdentifier(identifier),
                    database_marker: self.database_marker(),
                })
            }
            SemaCommand::Observe(query) => match self.observe(&query) {
                Some(entry) => SemaResponse::Observed(ObservedRecords {
                    record_set: RecordSet(entry),
                    database_marker: self.database_marker(),
                }),
                None => SemaResponse::Missed(ErrorReport {
                    error_message: ErrorMessage(String::from("no matching record")),
                    database_marker: self.database_marker(),
                }),
            },
        }
    }

    fn record(&mut self, entry: Entry) -> u64 {
        let identifier = self.next_identifier;
        self.next_identifier += 1;
        self.commit_sequence += 1;
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

    pub fn database_marker(&self) -> DatabaseMarker {
        DatabaseMarker {
            commit_sequence: CommitSequence(self.commit_sequence),
            state_digest: StateDigest(self.state_digest()),
        }
    }

    fn state_digest(&self) -> u64 {
        self.records
            .iter()
            .fold(self.commit_sequence, |digest, record| {
                digest
                    .wrapping_mul(31)
                    .wrapping_add(record.identifier)
                    .wrapping_add(record.entry.magnitude_weight())
            })
    }
}

impl Entry {
    pub fn matches(&self, query: &Query) -> bool {
        self.topic == query.topic && self.kind == query.kind
    }

    pub fn magnitude_weight(&self) -> u64 {
        self.magnitude.weight()
    }
}

impl Magnitude {
    pub fn weight(&self) -> u64 {
        match self {
            Self::Minimum => 1,
            Self::VeryLow => 2,
            Self::Low => 3,
            Self::Medium => 4,
            Self::High => 5,
            Self::VeryHigh => 6,
            Self::Maximum => 7,
        }
    }
}
