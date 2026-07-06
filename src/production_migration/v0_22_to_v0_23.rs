//! Spirit 0.22 to 0.23 production migration.
//!
//! The 0.23 public-intent schema removes active per-record `Certainty` and
//! embedded `Referents` from `Entry`. Historical stores still carry those fields
//! in rkyv bytes, so the frozen readers consume them here and intentionally
//! leave them out of the current `Entry`. Explicit referent registry rows are
//! migrated separately by the shared fold support.

use crate::schema::signal::{Description, Domains, Entry, Importance, Kind, Privacy, Referents};

use super::support::LegacyCertainty;

pub(super) struct PublicIntentEntryMigration {
    domains: Domains,
    kind: Kind,
    description: Description,
    legacy_certainty: LegacyCertainty,
    importance: Importance,
    privacy: Privacy,
    legacy_referents: Referents,
}

impl PublicIntentEntryMigration {
    pub(super) fn new(
        domains: Domains,
        kind: Kind,
        description: Description,
        legacy_certainty: LegacyCertainty,
        importance: Importance,
        privacy: Privacy,
        legacy_referents: Referents,
    ) -> Self {
        Self {
            domains,
            kind,
            description,
            legacy_certainty,
            importance,
            privacy,
            legacy_referents,
        }
    }

    pub(super) fn into_current(self) -> Entry {
        let Self {
            domains,
            kind,
            description,
            legacy_certainty,
            importance,
            privacy,
            legacy_referents,
        } = self;
        let _historical_fields = (legacy_certainty, legacy_referents);
        Entry {
            domains,
            kind,
            description,
            importance,
            privacy,
        }
    }
}
