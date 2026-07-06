//! The gated component-side mirror shipper.
//!
//! Spirit's durable store is a versioned sema-engine log (Spirit `iir4`); the
//! sema version-control system mirrors that log to a payload-blind remote
//! (`mirror`). This module is the OFF-by-default gate: a [`MirrorShipper`]
//! holds no shipper until an owner configures a meta `MirrorTarget`
//! ([`crate::schema::meta_signal::MirrorTarget`]). With no target, no shipper
//! exists and the daemon behaves exactly as a daemon with no mirroring at all
//! — no connection, no thread, no behavior change.
//!
//! When armed, the shipper holds a CLONE of the store's `Arc<sema_engine::Engine>`
//! (the very engine the working writes append to) plus the configured tailnet
//! address. After each durable working commit, the daemon drains the engine's
//! unshipped outbox through the reusable production `mirror::ComponentShipper`:
//! it sends the versioned-log suffix to the mirror ingress, and on a confirmed
//! head records `acknowledge_mirror` back into the shared engine, marking the
//! shipped history `ServerCommitted`.
//!
//! The contract and mirror-side shared-engine constructor live on their
//! respective main branches; the `mirror-shipper` feature is the opt-in gate
//! that pulls them into Spirit.

use std::{net::SocketAddr, sync::Arc};

use mirror::{ComponentShipper, ShipOutcome};
use sema_engine::{
    CommitSequence, Engine as SemaDatabase, EntryDigest, MirrorHead, VersionedStoreName,
};
use signal_mirror::{EntrySuffix, Input as MirrorInput, Output as MirrorOutput};
use thiserror::Error;

use crate::schema::{meta_signal::MirrorTarget, sema::RecordFamily};

/// The gate over the component mirror shipper. `Off` is the default and the
/// only state a daemon reaches without an owner `Configure` carrying a
/// `MirrorTarget::Address`; `Armed` holds the live shipper.
///
/// The shipper is a data-bearing runtime object — it owns the shared engine
/// handle, the configured address, and the store name it ships under — so its
/// methods live here rather than on a placeholder.
#[derive(Default)]
pub struct MirrorShipper {
    armed: Option<ComponentShipper>,
}

impl MirrorShipper {
    /// An unarmed gate: no mirror target configured, nothing ships.
    pub fn new() -> Self {
        Self::default()
    }

    /// An armed shipper at a known address over the store's shared engine
    /// handle — the propagation drain's construction path, which rebuilds its
    /// own shipper instance from the engine's configured mirror address
    /// rather than re-parsing a meta target.
    pub fn armed(engine: Arc<SemaDatabase>, address: SocketAddr) -> Self {
        Self {
            armed: Some(ComponentShipper::from_shared_engine(
                engine,
                address,
                VersionedStoreName::new(RecordFamily::STORE_NAME),
            )),
        }
    }

    /// Apply an owner-configured mirror target against the store's shared
    /// engine handle. `MirrorTarget::Address` arms the shipper at the parsed
    /// tailnet socket address; `MirrorTarget::Default` and a cleared target
    /// (`None`) disarm it. Re-arming replaces the prior shipper.
    pub fn configure(
        &mut self,
        target: Option<&MirrorTarget>,
        engine: Arc<SemaDatabase>,
    ) -> Result<(), MirrorShipperError> {
        self.armed = match target {
            Some(MirrorTarget::Address(address)) => {
                let text = address.payload().payload();
                let socket_address =
                    text.parse::<SocketAddr>()
                        .map_err(|source| MirrorShipperError::Address {
                            text: text.clone(),
                            message: source.to_string(),
                        })?;
                // The store hands its `Arc<sema_engine::Engine>` straight to
                // the shipper via `from_shared_engine`, so store and shipper
                // hold clones of ONE engine: the working writes the store
                // appends become the outbox this shipper ships, and
                // `acknowledge_mirror` flows back into the same engine.
                Some(ComponentShipper::from_shared_engine(
                    engine,
                    socket_address,
                    VersionedStoreName::new(RecordFamily::STORE_NAME),
                ))
            }
            Some(MirrorTarget::Default) | None => None,
        };
        Ok(())
    }

    /// Whether a mirror target is configured and the shipper is live.
    pub fn is_armed(&self) -> bool {
        self.armed.is_some()
    }

    /// The configured tailnet address, when armed.
    pub fn address(&self) -> Option<SocketAddr> {
        self.armed
            .as_ref()
            .map(|shipper| shipper.client().address())
    }

    /// Drain the engine's unshipped outbox to the configured mirror. A no-op
    /// (returns `Ok(None)`) when unarmed, so the daemon's post-commit hook can
    /// call this unconditionally and pay nothing when mirroring is off. When
    /// armed it ships the versioned-log suffix and, on a confirmed head,
    /// records `acknowledge_mirror` into the shared engine.
    pub async fn ship_unshipped(&self) -> Result<Option<ShipOutcome>, MirrorShipperError> {
        match &self.armed {
            Some(shipper) => Ok(Some(shipper.ship_unshipped().await?)),
            None => Ok(None),
        }
    }

    /// Ship the unshipped suffix UP TO AND INCLUDING the authorized entry —
    /// the batch one cluster authorization covers — and acknowledge the
    /// outbox cursor to exactly that head. Entries the store committed AFTER
    /// the authorized head stay unshipped (they await their own
    /// authorization): only the authorized suffix ever leaves the node,
    /// fail-closed. A no-op when unarmed, and a typed no-op when the
    /// authorized entry is no longer in the unshipped suffix (an idempotent
    /// re-drain after acknowledgement).
    pub async fn ship_authorized_suffix(
        &self,
        authorized: &EntryDigest,
    ) -> Result<Option<ShipOutcome>, MirrorShipperError> {
        let Some(shipper) = &self.armed else {
            return Ok(None);
        };
        let outbox = shipper.engine().unshipped_outbox().map_err(|source| {
            MirrorShipperError::AuthorizedSuffix {
                detail: source.to_string(),
            }
        })?;
        let Some(cap_index) = outbox
            .iter()
            .position(|row| row.entry_digest() == *authorized)
        else {
            // Nothing unshipped up to the authorized head: it was already
            // acknowledged by an earlier pass. Idempotent no-op.
            return Ok(Some(ShipOutcome::AlreadyCommitted {
                head: shipper.engine().mirror_head().map_err(|source| {
                    MirrorShipperError::AuthorizedSuffix {
                        detail: source.to_string(),
                    }
                })?,
            }));
        };
        let capped = &outbox[..=cap_index];
        let first = capped[0].commit_sequence();
        let replayed = shipper
            .engine()
            .versioned_replay_from_sequence(first)
            .map_err(|source| MirrorShipperError::AuthorizedSuffix {
                detail: source.to_string(),
            })?;
        let entries = replayed
            .iter()
            .take(capped.len())
            .map(|entry| shipper.envelope_for_entry(entry))
            .collect::<Result<Vec<_>, _>>()?;
        if entries.len() != capped.len() {
            return Err(MirrorShipperError::AuthorizedSuffix {
                detail: format!(
                    "outbox names {} authorized rows but the replay yields {} entries",
                    capped.len(),
                    entries.len()
                ),
            });
        }
        let output = shipper
            .client()
            .exchange(MirrorInput::Append(EntrySuffix::from_entries(
                shipper.store_name().clone(),
                shipper.expected_head()?,
                entries,
            )))
            .await?;
        let receipt = match output {
            MirrorOutput::Appended(receipt) => receipt,
            other => {
                return Err(MirrorShipperError::AuthorizedSuffix {
                    detail: format!("mirror refused the authorized suffix: {other:?}"),
                });
            }
        };
        let head = MirrorHead::new(
            CommitSequence::new(*receipt.head.sequence.payload()),
            EntryDigest::new(*receipt.head.digest.payload().payload()),
        );
        shipper.engine().acknowledge_mirror(head).map_err(|source| {
            MirrorShipperError::AuthorizedSuffix {
                detail: source.to_string(),
            }
        })?;
        Ok(Some(ShipOutcome::Shipped { head }))
    }

    /// Publish the store's latest local checkpoint to the configured mirror —
    /// the portable restore body a fresh store fetches alongside the shipped
    /// log suffix. A no-op (returns `Ok(false)`) when unarmed. The store must
    /// have written a local checkpoint first; this only relays it.
    pub async fn publish_checkpoint(&self) -> Result<bool, MirrorShipperError> {
        match &self.armed {
            Some(shipper) => {
                shipper.publish_latest_checkpoint().await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

impl std::fmt::Debug for MirrorShipper {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MirrorShipper")
            .field("armed", &self.is_armed())
            .field("address", &self.address())
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum MirrorShipperError {
    #[error("mirror target address {text} is not a socket address: {message}")]
    Address { text: String, message: String },
    #[error("mirror shipper transport error: {0}")]
    Ship(#[from] mirror::Error),
    #[error("authorized-suffix ship failed: {detail}")]
    AuthorizedSuffix { detail: String },
}
