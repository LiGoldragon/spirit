use std::fmt;

use crate::TraceEvent;
pub use triad_runtime::trace::{TraceError, TraceEventFrame, TraceSocketPath};

pub type TraceClient = triad_runtime::trace::TraceClient<TraceEvent>;
pub type TraceLog = triad_runtime::trace::TraceLog<TraceEvent>;
pub type TraceSocketListener = triad_runtime::trace::TraceSocketListener<TraceEvent>;

impl TraceEventFrame for TraceEvent {
    fn to_trace_archive(&self) -> Result<Vec<u8>, TraceError> {
        rkyv::to_bytes::<rkyv::rancor::Error>(self)
            .map(|archive| archive.to_vec())
            .map_err(|_| TraceError::ArchiveEncode)
    }

    fn from_trace_archive(archive: &[u8]) -> Result<Self, TraceError> {
        rkyv::from_bytes::<Self, rkyv::rancor::Error>(archive)
            .map_err(|_| TraceError::ArchiveDecode)
    }
}

impl fmt::Display for TraceEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}
