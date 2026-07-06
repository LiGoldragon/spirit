use crate::schema::signal::Magnitude;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, PartialEq, Eq)]
pub(super) struct LegacyCertainty(Magnitude);

#[cfg(test)]
impl LegacyCertainty {
    pub(super) fn new(payload: Magnitude) -> Self {
        Self(payload)
    }
}
