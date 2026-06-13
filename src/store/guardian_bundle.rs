//! The guardian record bundle: a dedup accumulator for the records a guardian
//! operation needs in context.
//!
//! It is data-bearing — it cannot dedup without the `seen_identifiers` set,
//! and it cannot return a record set without its accumulated `records`. Each
//! insert is keyed on the record identifier so the same record gathered from
//! several context queries lands in the bundle exactly once.

use std::collections::BTreeSet;

use crate::schema::signal::{ObservedRecord, RecordSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GuardianRecordBundle {
    seen_identifiers: BTreeSet<String>,
    records: Vec<ObservedRecord>,
}

impl GuardianRecordBundle {
    pub(super) fn new() -> Self {
        Self {
            seen_identifiers: BTreeSet::new(),
            records: Vec::new(),
        }
    }

    pub(super) fn insert(&mut self, record: ObservedRecord) {
        if self
            .seen_identifiers
            .insert(record.record_identifier.payload().clone())
        {
            self.records.push(record);
        }
    }

    pub(super) fn extend(&mut self, record_set: RecordSet) {
        for record in record_set.into_payload() {
            self.insert(record);
        }
    }

    pub(super) fn into_record_set(self) -> RecordSet {
        RecordSet::new(self.records)
    }
}
