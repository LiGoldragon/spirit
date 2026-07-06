//! The propagation drain (§3.5): the single supervised background worker that
//! decouples the working reply from cluster authorization.
//!
//! Each durable working commit sends the drain a "head advanced" mail
//! ([`crate::Engine::notify_head_advanced`]); the drain — never the working
//! reply path — runs the authorize-then-ship sequence for the CURRENT
//! unshipped suffix, one outstanding authorization at a time. On
//! [`GateDecision::Authorized`] it ships the suffix UP TO the authorized
//! digest and acknowledges the outbox cursor to it; any other decision leaves
//! the outbox intact, and the next commit's mail re-attempts with the
//! then-current head (the criome-side catch-up rule makes that safe even
//! though the head moved). No poll loop exists anywhere: the drain runs only
//! when a commit pushes mail, and coalesces bursts into single passes.
//!
//! For deterministic tests the drain is drivable directly through
//! [`crate::Engine::drain_propagation_once`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sema_engine::{Engine as SemaDatabase, EntryDigest};

use crate::criome_gate::{ClusterAuthorizer, GateDecision, LocalHeadCapture};
use crate::engine::GateAndShipError;
use crate::shipper::MirrorShipper;

/// The drain: the cluster authorizer, a shared handle to the durable store
/// it captures heads from and acknowledges cursors into, and the (possibly
/// unarmed) mirror shipper for the authorized suffix. Shared behind an `Arc`
/// between the engine (the direct test drive) and the mail-spawned passes.
pub struct PropagationDrain {
    authorizer: ClusterAuthorizer,
    database: Arc<SemaDatabase>,
    shipper: MirrorShipper,
    /// The one-outstanding-authorization serialization: a pass holds this
    /// across its authorize-then-ship sequence.
    running: tokio::sync::Mutex<()>,
    /// The coalescing mail flag: a commit landing while a pass runs marks it,
    /// and the running pass re-drains before releasing.
    pending: AtomicBool,
}

impl PropagationDrain {
    pub fn new(
        authorizer: ClusterAuthorizer,
        database: Arc<SemaDatabase>,
        shipper: MirrorShipper,
    ) -> Self {
        Self {
            authorizer,
            database,
            shipper,
            running: tokio::sync::Mutex::new(()),
            pending: AtomicBool::new(false),
        }
    }

    /// One authorize-then-ship pass over the current head: capture the head
    /// of the UNSHIPPED suffix, ask the cluster authorizer, and on Granted
    /// ship the suffix up to exactly that head. Nothing unshipped — an empty
    /// log, or a cursor already acknowledged to the head — is a no-op `None`:
    /// an idle mail pass (a read input, the coalescing re-pass right after a
    /// granted ship) never re-asks the cluster for a head it has nothing to
    /// ship under (audit F1 hygiene; the criome bridge independently
    /// re-grants a committed head, so the grant-then-ship-failure re-ask —
    /// where the suffix IS still unshipped — stays live).
    pub async fn drain_once(&self) -> Result<Option<GateDecision>, GateAndShipError> {
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
    /// holding the serialization lock, coalescing every burst of commits into
    /// as few passes as the in-flight authorizations allow. A caller that
    /// loses the lock returns immediately — the holder observes the pending
    /// flag and re-drains, and the post-release re-check closes the window
    /// between its last look and the release.
    pub async fn drain_serialized(self: Arc<Self>) {
        self.pending.store(true, Ordering::SeqCst);
        loop {
            let Ok(guard) = self.running.try_lock() else {
                return;
            };
            while self.pending.swap(false, Ordering::SeqCst) {
                if let Err(error) = self.drain_once().await {
                    // A machinery fault holds the head (nothing shipped); the
                    // local commit already stands. Loud, never silent.
                    eprintln!("spirit propagation drain pass failed (head held): {error}");
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
    /// acknowledged to the mirror (nothing to authorize, nothing to ship).
    /// When anything is unshipped, this is exactly the versioned-log head:
    /// the outbox is the contiguous suffix from the shipped cursor to the
    /// head.
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
