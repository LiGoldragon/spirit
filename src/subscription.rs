//! Spirit's stream subscription token.
//!
//! The live-subscription registry, the per-subscriber writer map, and the
//! publish path are EMITTED into `src/schema/daemon.rs` by schema-rust-next's
//! daemon emitter (option B): a declared stream becomes emitted daemon
//! plumbing. The hand-written `SubscriptionHub` that previously lived here is
//! retired. Spirit keeps only the token newtype that bridges the
//! schema-emitted `SubscriptionToken` integer to the runtime-owned
//! `triad_runtime::SubscriptionToken` trait the emitted registry is generic over.

use signal_frame::SubscriptionTokenInner;
use triad_runtime::SubscriptionToken;

use crate::schema::signal::SubscriptionToken as SignalSubscriptionToken;

/// Spirit's registry key: the schema-emitted subscription token wrapped as the
/// runtime-owned `SubscriptionTokenInner` so the emitted
/// `SubscriptionRegistry<IntentSubscriptionToken, Query>` can store it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct IntentSubscriptionToken(SubscriptionTokenInner);

impl SubscriptionToken for IntentSubscriptionToken {
    fn from_inner(inner: SubscriptionTokenInner) -> Self {
        Self(inner)
    }

    fn into_inner(self) -> SubscriptionTokenInner {
        self.0
    }
}

impl IntentSubscriptionToken {
    pub fn from_signal_token(token: SignalSubscriptionToken) -> Self {
        Self(SubscriptionTokenInner::new(token.into_payload()))
    }
}
