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
//! RE-LAND STATUS (designer report 669, slice P1): this module is re-landed
//! from the dropped `store-decomposition` history onto current spirit main, but
//! the `mirror-shipper` feature does NOT yet compile. Two upstream gaps block
//! it, neither fixable from this spirit-only slice:
//!
//!  1. `crate::schema::meta_signal::MirrorTarget` (and `MirrorAddress`,
//!     `MirrorAddressText`, plus the `mirror_target` field on `ConfigureRequest`
//!     / `ConfigureReceipt`) no longer exist: current spirit re-exports
//!     `meta_signal` from the external `meta-signal-spirit` crate, whose
//!     `ConfigureRequest(ArchiveDatabaseTarget)` carries no mirror target. The
//!     `MirrorTarget` schema noun must be re-added to the `meta-signal-spirit`
//!     contract (a separate triad repo) and regenerated into spirit.
//!  2. The only `mirror::ComponentShipper` that accepts a shared
//!     `Arc<sema_engine::Engine>` lives on mirror's `arc-shipper` branch (mirror
//!     `main` regressed it to take `Engine` by value, which a store keeping its
//!     engine behind an `Arc` cannot satisfy). But `arc-shipper` itself fails to
//!     compile against today's tree: it pins a stale `nota-next` (0.4.0 via
//!     `structural-shape-extension`) while current `signal-mirror` main pins
//!     `nota-next` 0.5.0, so mirror's `client.rs` NOTA codec no longer lines up
//!     with the regenerated `signal-mirror` types. The `Arc<Engine>` shipper
//!     must be forward-ported onto mirror main's current `nota-next` base (a
//!     mirror-repo change).

use std::{net::SocketAddr, sync::Arc};

use mirror::{ComponentShipper, ShipOutcome};
use sema_engine::{Engine as SemaDatabase, VersionedStoreName};
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
                Some(ComponentShipper::new(
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
        self.armed.as_ref().map(|shipper| shipper.client().address())
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
}
