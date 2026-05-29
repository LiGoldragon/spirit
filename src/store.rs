use std::{fmt, fs, path::PathBuf};

use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};

use crate::{
    CommitSequence, DatabaseMarker, Entry, ErrorMessage, ErrorReport, Magnitude, ObservedRecords,
    Query, RecordIdentifier, RecordSet, RemoveReceipt, SemaEngine, SemaInput, SemaOutput,
    SemaReceipt, StateDigest, schema::lib::sema as sema_plane,
};

/// redb table of durable records: identifier -> rkyv-archived `Entry`.
const RECORDS: TableDefinition<u64, &[u8]> = TableDefinition::new("records");
/// redb table of the SEMA commit ledger: a single-key counter pair.
const LEDGER: TableDefinition<&str, u64> = TableDefinition::new("ledger");
/// Ledger key holding the identifier issued to the next durable write.
const NEXT_IDENTIFIER_KEY: &str = "next-identifier";
/// Ledger key holding the count of durable commits applied so far.
const COMMIT_SEQUENCE_KEY: &str = "commit-sequence";

/// The SEMA durable store: a real redb database written to a `*.sema`
/// file.
///
/// SEMA means database work — writing to the durable state file (records
/// 1007/1008). Each `Record` operation is a redb write transaction; each
/// `Observe` is a redb read transaction. The commit sequence and the
/// next-identifier counter persist in the database itself, so a store
/// reopened from the same `.sema` path resumes exactly where it left off.
#[derive(Debug)]
pub struct Store {
    database: Database,
    path: PathBuf,
}

impl SemaEngine for Store {
    fn apply(
        &mut self,
        command: sema_plane::Sema<sema_plane::Input>,
    ) -> sema_plane::Sema<sema_plane::Output> {
        let origin_route = command.origin_route();
        let output = match command.into_root() {
            SemaInput::Record(entry) => match self.record(entry) {
                Ok(identifier) => SemaOutput::Recorded(SemaReceipt {
                    record_identifier: RecordIdentifier(identifier),
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaOutput::Missed(ErrorReport {
                    error_message: ErrorMessage(error.to_string()),
                    database_marker: self.database_marker(),
                }),
            },
            SemaInput::Observe(query) => match self.observe(&query) {
                Ok(entries) if !entries.is_empty() => SemaOutput::Observed(ObservedRecords {
                    record_set: RecordSet(entries),
                    database_marker: self.database_marker(),
                }),
                Ok(_) => SemaOutput::Missed(ErrorReport {
                    error_message: ErrorMessage(String::from("no matching record")),
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaOutput::Missed(ErrorReport {
                    error_message: ErrorMessage(error.to_string()),
                    database_marker: self.database_marker(),
                }),
            },
            SemaInput::Remove(record_identifier) => match self.remove(record_identifier.0) {
                Ok(true) => SemaOutput::Removed(RemoveReceipt {
                    record_identifier,
                    database_marker: self.database_marker(),
                }),
                Ok(false) => SemaOutput::Missed(ErrorReport {
                    error_message: ErrorMessage(String::from("record not found")),
                    database_marker: self.database_marker(),
                }),
                Err(error) => SemaOutput::Missed(ErrorReport {
                    error_message: ErrorMessage(error.to_string()),
                    database_marker: self.database_marker(),
                }),
            },
        };
        output.with_origin_route(origin_route)
    }
}

impl Store {
    /// Open or create the durable SEMA database at `path`.
    ///
    /// A fresh file is created with empty ledger counters; an existing
    /// file resumes its persisted commit sequence and identifier counter.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let database = if path.exists() {
            Database::open(&path)?
        } else {
            Database::create(&path)?
        };
        let store = Self { database, path };
        store.ensure_tables()?;
        Ok(store)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn ensure_tables(&self) -> Result<(), StoreError> {
        let transaction = self.database.begin_write()?;
        {
            transaction.open_table(RECORDS)?;
            let mut ledger = transaction.open_table(LEDGER)?;
            if ledger.get(NEXT_IDENTIFIER_KEY)?.is_none() {
                ledger.insert(NEXT_IDENTIFIER_KEY, 1_u64)?;
            }
            if ledger.get(COMMIT_SEQUENCE_KEY)?.is_none() {
                ledger.insert(COMMIT_SEQUENCE_KEY, 0_u64)?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    fn record(&self, entry: Entry) -> Result<u64, StoreError> {
        let archive =
            rkyv::to_bytes::<rkyv::rancor::Error>(&entry).map_err(|_| StoreError::ArchiveEncode)?;
        let transaction = self.database.begin_write()?;
        let identifier;
        {
            let mut ledger = transaction.open_table(LEDGER)?;
            identifier = ledger
                .get(NEXT_IDENTIFIER_KEY)?
                .map(|value| value.value())
                .unwrap_or(1);
            let commit_sequence = ledger
                .get(COMMIT_SEQUENCE_KEY)?
                .map(|value| value.value())
                .unwrap_or(0);
            ledger.insert(NEXT_IDENTIFIER_KEY, identifier + 1)?;
            ledger.insert(COMMIT_SEQUENCE_KEY, commit_sequence + 1)?;
            let mut records = transaction.open_table(RECORDS)?;
            records.insert(identifier, &archive[..])?;
        }
        transaction.commit()?;
        Ok(identifier)
    }

    fn observe(&self, query: &Query) -> Result<Vec<Entry>, StoreError> {
        let transaction = self.database.begin_read()?;
        let records = transaction.open_table(RECORDS)?;
        let mut matches = Vec::new();
        for row in records.iter()? {
            let (_, archive) = row?;
            let entry = rkyv::from_bytes::<Entry, rkyv::rancor::Error>(archive.value())
                .map_err(|_| StoreError::ArchiveDecode)?;
            if entry.matches(query) {
                matches.push(entry);
            }
        }
        Ok(matches)
    }

    fn remove(&self, identifier: u64) -> Result<bool, StoreError> {
        let transaction = self.database.begin_write()?;
        let removed;
        {
            let mut records = transaction.open_table(RECORDS)?;
            removed = records.remove(identifier)?.is_some();
            if removed {
                let mut ledger = transaction.open_table(LEDGER)?;
                let commit_sequence = ledger
                    .get(COMMIT_SEQUENCE_KEY)?
                    .map(|value| value.value())
                    .unwrap_or(0);
                ledger.insert(COMMIT_SEQUENCE_KEY, commit_sequence + 1)?;
            }
        }
        transaction.commit()?;
        Ok(removed)
    }

    pub fn len(&self) -> usize {
        self.committed_record_count().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn committed_record_count(&self) -> Result<usize, StoreError> {
        let transaction = self.database.begin_read()?;
        let records = transaction.open_table(RECORDS)?;
        Ok(records.len()? as usize)
    }

    /// The SEMA commit marker: the persisted commit sequence plus a real
    /// content hash of the committed records.
    pub fn database_marker(&self) -> DatabaseMarker {
        DatabaseMarker {
            commit_sequence: CommitSequence(self.commit_sequence().unwrap_or(0)),
            state_digest: StateDigest(self.state_digest().unwrap_or(0)),
        }
    }

    fn commit_sequence(&self) -> Result<u64, StoreError> {
        let transaction = self.database.begin_read()?;
        let ledger = transaction.open_table(LEDGER)?;
        Ok(ledger
            .get(COMMIT_SEQUENCE_KEY)?
            .map(|value| value.value())
            .unwrap_or(0))
    }

    /// A content-addressed digest of committed state: blake3 over each
    /// record's `(identifier, archived bytes)`, folded with the commit
    /// sequence, reduced to the schema's `Integer` digest width. An empty
    /// store (no committed records) digests to zero, so a marker taken
    /// before any write reads `(0, 0)`.
    fn state_digest(&self) -> Result<u64, StoreError> {
        let transaction = self.database.begin_read()?;
        let records = transaction.open_table(RECORDS)?;
        if records.is_empty()? {
            return Ok(0);
        }
        let ledger = transaction.open_table(LEDGER)?;
        let commit_sequence = ledger
            .get(COMMIT_SEQUENCE_KEY)?
            .map(|value| value.value())
            .unwrap_or(0);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&commit_sequence.to_le_bytes());
        for row in records.iter()? {
            let (identifier, archive) = row?;
            hasher.update(&identifier.value().to_le_bytes());
            hasher.update(archive.value());
        }
        let digest = hasher.finalize();
        let mut head = [0_u8; 8];
        head.copy_from_slice(&digest.as_bytes()[..8]);
        Ok(u64::from_le_bytes(head))
    }
}

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    /// Any redb-level failure (open, transaction, table, storage, commit).
    /// Boxed because redb's error types are large; the boxed `redb::Error`
    /// preserves the specific failure for `Display` and matching.
    Database(Box<redb::Error>),
    ArchiveEncode,
    ArchiveDecode,
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "sema store IO error: {error}"),
            Self::Database(error) => write!(formatter, "sema database error: {error}"),
            Self::ArchiveEncode => formatter.write_str("failed to encode record rkyv archive"),
            Self::ArchiveDecode => formatter.write_str("failed to decode record rkyv archive"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<redb::DatabaseError> for StoreError {
    fn from(value: redb::DatabaseError) -> Self {
        Self::Database(Box::new(value.into()))
    }
}

impl From<redb::TransactionError> for StoreError {
    fn from(value: redb::TransactionError) -> Self {
        Self::Database(Box::new(value.into()))
    }
}

impl From<redb::TableError> for StoreError {
    fn from(value: redb::TableError) -> Self {
        Self::Database(Box::new(value.into()))
    }
}

impl From<redb::StorageError> for StoreError {
    fn from(value: redb::StorageError) -> Self {
        Self::Database(Box::new(value.into()))
    }
}

impl From<redb::CommitError> for StoreError {
    fn from(value: redb::CommitError) -> Self {
        Self::Database(Box::new(value.into()))
    }
}

impl Entry {
    pub fn matches(&self, query: &Query) -> bool {
        query.matches(self)
    }

    pub fn magnitude_weight(&self) -> u64 {
        self.magnitude.weight()
    }
}

impl Query {
    pub fn matches(&self, entry: &Entry) -> bool {
        self.topic_match.matches(&entry.topics)
            && self.kind.as_ref().is_none_or(|kind| &entry.kind == kind)
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
