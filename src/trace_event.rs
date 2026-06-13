use crate::{
    engine::SignalObjectName,
    schema::{nexus::NexusObjectName, sema::SemaObjectName},
};

#[cfg_attr(
    feature = "nota-text",
    derive(nota_next::NotaDecode, nota_next::NotaEncode)
)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectName {
    Signal(SignalObjectName),
    Nexus(NexusObjectName),
    Sema(SemaObjectName),
}

#[cfg_attr(
    feature = "nota-text",
    derive(nota_next::NotaDecode, nota_next::NotaEncode)
)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceEvent(pub ObjectName);

impl ObjectName {
    pub fn name(self) -> &'static str {
        match self {
            Self::Signal(object_name) => object_name.name(),
            Self::Nexus(object_name) => object_name.name(),
            Self::Sema(object_name) => object_name.name(),
        }
    }
}

impl TraceEvent {
    pub fn new(object_name: ObjectName) -> Self {
        Self(object_name)
    }

    pub fn object_name(&self) -> ObjectName {
        self.0
    }

    pub fn name(&self) -> &'static str {
        self.0.name()
    }
}
