//! The ship drain (§3.6): distribution of ACCEPTED state, plus the residue
//! reconciler.
//!
//! Under the everywhere-gate, acceptance happens on the intake path — a
//! head-advancing operation materializes only after the cluster grant — so
//! everything in an Enabled spirit's log is cluster-authorized by
//! construction. This drain therefore never gates: on each materialization it
//! receives "head advanced" mail ([`crate::Engine::notify_head_advanced`])
//! and ships the unshipped outbox suffix. Spirit does not durably retain
//! grants; at ship time the drain re-asks its criome for the suffix head,
//! which short-circuits to the immediate re-grant of the standing committed
//! head — a local socket round-trip, no cluster round — making
//! grant-then-ship-failure retries trivially safe: the suffix waits, the next
//! mail re-fetches, ships, and acknowledges the cursor.
//!
//! The ONE case where this drain asks for a genuinely NEW round is
//! disabled-era residue (§3.4): an unshipped suffix whose head the cluster
//! has never granted, recorded while the gate was Disabled. Enabling the gate
//! fires one reconcile mail; the drain proposes the current head, the batch
//! grant covers the residue transitively (the hash chain fixes every entry
//! beneath the head), and the suffix ships. A drain pass shares the intake's
//! first-in first-out advance gate, keeping at most one proposal outstanding
//! ahead of the cluster head.
//!
//! No poll loop exists anywhere: the drain runs only when a materialization
//! or reconcile pushes mail, and coalesces bursts into single passes. For
//! deterministic tests the drain is drivable directly through
//! [`crate::Engine::drain_propagation_once`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sema_engine::{Engine as SemaDatabase, EntryDigest};

use crate::criome_gate::{ClusterAuthorizer, GateDecision, LocalHeadCapture};
use crate::engine::GateAndShipError;
use crate::shipper::MirrorShipper;

/// The drain: the cluster authorizer (the ship-time re-grant fetch and the
/// residue round), a shared handle to the durable store it captures suffix
/// heads from and acknowledges cursors into, the (possibly unarmed) mirror
/// shipper, and the intake's shared advance gate. Shared behind an `Arc`
/// between the engine (the direct test drive) and the mail-spawned passes.
pub struct PropagationDrain {
    authorizer: ClusterAuthorizer,
    database: Arc<SemaDatabase>,
    shipper: MirrorShipper,
    /// The intake's first-in first-out advance gate (§3.5.3), shared so a
    /// residue reconcile round never runs beside an in-flight intake round.
    advance_gate: Arc<tokio::sync::Mutex<()>>,
    /// The one-outstanding-pass serialization: a pass holds this across its
    /// fetch-grant-then-ship sequence.
    running: tokio::sync::Mutex<()>,
    /// The coalescing mail flag: a materialization landing while a pass runs
    /// marks it, and the running pass re-drains before releasing.
    pending: AtomicBool,
}

impl PropagationDrain {
    pub fn new(
        authorizer: ClusterAuthorizer,
        database: Arc<SemaDatabase>,
        shipper: MirrorShipper,
        advance_gate: Arc<tokio::sync::Mutex<()>>,
    ) -> Self {
        Self {
            authorizer,
            database,
            shipper,
            advance_gate,
            running: tokio::sync::Mutex::new(()),
            pending: AtomicBool::new(false),
        }
    }

    /// One ship pass over the current suffix: capture the head of the
    /// UNSHIPPED suffix, fetch its grant from the local criome, and on
    /// Granted ship the suffix up to exactly that head. In the steady state
    /// the suffix head was accepted at intake and the ask short-circuits to
    /// the immediate re-grant of the standing committed head; for
    /// disabled-era residue this is the ONE genuinely new (batch) round.
    /// Nothing unshipped — an empty log, or a cursor already acknowledged to
    /// the head — is a no-op `None`: an idle mail pass never asks criome for
    /// a head it has nothing to ship under. The pass runs under the shared
    /// advance gate so at most one proposal is outstanding.
    pub async fn drain_once(&self) -> Result<Option<GateDecision>, GateAndShipError> {
        let _advance_turn = self.advance_gate.lock().await;
        let Some(head_digest) = self.unshipped_suffix_head()? else {
            return Ok(None);
        };
        let capture = LocalHeadCapture::spirit_head(head_digest);
        let decision = self.authorizer.authorize_head(&capture).await?;
        if decision.ships() {
            self.shipper
                .ship_authorized_suffix(capture.head_digest())
                .await?;
        }
        Ok(Some(decision))
    }

    /// The mail-driven entry: mark a drain pending and run passes while
    /// holding the serialization lock, coalescing every burst of
    /// materializations into as few passes as the in-flight grant fetches
    /// allow. A caller that loses the lock returns immediately — the holder
    /// observes the pending flag and re-drains, and the post-release
    /// re-check closes the window between its last look and the release.
    pub async fn drain_serialized(self: Arc<Self>) {
        self.pending.store(true, Ordering::SeqCst);
        loop {
            let Ok(guard) = self.running.try_lock() else {
                return;
            };
            while self.pending.swap(false, Ordering::SeqCst) {
                if let Err(error) = self.drain_once().await {
                    // A machinery fault withholds the ship; the accepted
                    // state stands in the log and the suffix waits for the
                    // next mail. Loud, never silent.
                    eprintln!("spirit ship drain pass failed (suffix waits): {error}");
                }
            }
            drop(guard);
            if !self.pending.load(Ordering::SeqCst) {
                return;
            }
        }
    }

    /// The head of the UNSHIPPED suffix straight from the shared durable
    /// store's outbox — `None` when every committed entry is already
    /// acknowledged to the mirror (nothing to ship). When anything is
    /// unshipped, this is exactly the versioned-log head: the outbox is the
    /// contiguous suffix from the shipped cursor to the head.
    fn unshipped_suffix_head(&self) -> Result<Option<EntryDigest>, GateAndShipError> {
        Ok(self
            .database
            .unshipped_outbox()
            .map_err(crate::store::StoreError::from)?
            .last()
            .map(sema_engine::OutboxEntry::entry_digest))
    }
}

impl std::fmt::Debug for PropagationDrain {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PropagationDrain")
            .field("authorizer", &self.authorizer)
            .field("shipper", &self.shipper)
            .finish()
    }
}
