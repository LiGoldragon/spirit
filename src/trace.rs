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

#[cfg(feature = "nota-text")]
impl std::fmt::Display for TraceEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&<Self as nota_next::NotaEncode>::to_nota(self))
    }
}

#[cfg(feature = "nota-text")]
impl std::str::FromStr for TraceEvent {
    type Err = nota_next::NotaDecodeError;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        nota_next::NotaSource::new(source).parse::<Self>()
    }
}
