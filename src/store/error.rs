//! The store's typed error and the spirit-specific remediation tail.

use std::fmt;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(
        "sema database engine error: {engine_error}{}",
        DatabaseRemediation(engine_error)
    )]
    Database {
        #[from]
        engine_error: sema_engine::Error,
    },

    #[error("failed to encode record rkyv archive")]
    ArchiveEncode,

    #[error("failed to decode rkyv archive: {message}")]
    ArchiveDecode { message: String },

    #[error("restored mirror head mismatch: expected {expected:?}, restored {restored:?}")]
    MirrorRestoreHeadMismatch {
        expected: sema_engine::EntryDigest,
        restored: sema_engine::EntryDigest,
    },

    #[error("failed to mint record identifier: {0}")]
    IdentifierMint(String),

    #[error("unregistered referent: {0}")]
    UnregisteredReferent(String),

    #[error("referent name already registered under another canonical referent: {0}")]
    ReferentNameConflict(String),

    #[error("referent name is not lowercase kebab-case: {0}")]
    NonKebabReferent(String),

    #[error("duplicate record vanished during guardian proposal handling: {0}")]
    DuplicateRecordVanished(String),
}

/// The spirit-specific remediation tail appended to a wrapped engine error's
/// message. The engine reports a storage-layout mismatch in its own generic
/// terms ("rebuilt through checkpoint import or versioned replay"); spirit
/// names the concrete tool — `spirit-migrate-store` — that folds a previous
/// store forward. The layout numbers stay the engine's own runtime-rendered
/// `{stored}`/`{expected}` values, so this tail carries no literal layout
/// number that could drift on the next engine layout bump.
struct DatabaseRemediation<'engine_error>(&'engine_error sema_engine::Error);

impl fmt::Display for DatabaseRemediation<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            sema_engine::Error::StorageLayoutMismatch { .. } => {
                formatter.write_str("; run spirit-migrate-store to fold this store forward")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StoreError;

    /// A storage-layout mismatch surfaced as a `StoreError::Database` names the
    /// concrete spirit remediation tool, and the layout numbers in the message
    /// are the engine's own runtime values rather than a spirit literal.
    #[test]
    fn layout_mismatch_database_error_names_the_migration_tool() {
        let error = StoreError::Database {
            engine_error: sema_engine::Error::StorageLayoutMismatch {
                stored: 4,
                expected: 5,
            },
        };
        let message = error.to_string();
        assert!(
            message.contains("run spirit-migrate-store"),
            "layout-mismatch message must name the remediation tool, got: {message}",
        );
        assert!(
            message.contains("layout 4") && message.contains("layout 5"),
            "layout numbers come from the engine's own stored/expected values, got: {message}",
        );
    }

    /// A non-layout database error carries no remediation tail: the tool only
    /// helps with a layout mismatch.
    #[test]
    fn non_layout_database_error_has_no_remediation_tail() {
        let error = StoreError::Database {
            engine_error: sema_engine::Error::RecordNotFound {
                table: String::from("records"),
                key: String::from("missing"),
            },
        };
        let message = error.to_string();
        assert!(
            !message.contains("spirit-migrate-store"),
            "only a layout mismatch earns the remediation tail, got: {message}",
        );
    }
}
