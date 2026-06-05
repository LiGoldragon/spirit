use std::{collections::HashMap, io::Write, os::unix::net::UnixStream, sync::Mutex};

use signal_frame::{SessionEpoch, ShortHeader, SubscriptionTokenInner};
use thiserror::Error;
use triad_runtime::{
    FrameBody, FrameError, LengthPrefixedCodec, SubscriptionEventPublisher, SubscriptionRegistry,
    SubscriptionToken,
};

use crate::schema::signal::{
    Input, IntentEvent, Output, Query, SignalFrameError,
    SubscriptionToken as SignalSubscriptionToken, short_header,
};

#[derive(Debug, Error)]
pub enum SubscriptionError {
    #[error("subscription IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("subscription frame error: {0}")]
    Frame(#[from] FrameError),

    #[error("subscription signal-frame archive error: {0}")]
    SignalFrameArchive(#[from] signal_frame::FrameError),

    #[error("subscription signal frame error: {0}")]
    SignalFrame(#[from] SignalFrameError),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct IntentSubscriptionToken(SubscriptionTokenInner);

#[derive(Debug, Default)]
pub struct SubscriptionHub {
    state: Mutex<SubscriptionState>,
}

#[derive(Debug)]
struct SubscriptionState {
    registry: SubscriptionRegistry<IntentSubscriptionToken, Query>,
    writers: HashMap<IntentSubscriptionToken, UnixStream>,
    publisher: SubscriptionEventPublisher<Input, Output, IntentEvent>,
}

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
        Self(SubscriptionTokenInner::new(token))
    }
}

impl Default for SubscriptionState {
    fn default() -> Self {
        Self {
            registry: SubscriptionRegistry::new(),
            writers: HashMap::new(),
            publisher: SubscriptionEventPublisher::acceptor(
                ShortHeader::new(short_header::OUTPUT_EVENT),
                SessionEpoch::new(1),
            ),
        }
    }
}

impl SubscriptionHub {
    pub fn register(&self, token: SignalSubscriptionToken, filter: Query, writer: UnixStream) {
        let token = IntentSubscriptionToken::from_signal_token(token);
        let mut state = self.state.lock().expect("subscription state lock");
        state.registry.register_token(token, filter);
        state.writers.insert(token, writer);
    }

    pub fn publish(&self, event: IntentEvent) -> Result<usize, SubscriptionError> {
        self.state
            .lock()
            .expect("subscription state lock")
            .publish(event)
    }

    pub fn len(&self) -> usize {
        self.state.lock().expect("subscription state lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SubscriptionState {
    fn publish(&mut self, event: IntentEvent) -> Result<usize, SubscriptionError> {
        let mut frames = Vec::new();
        self.registry
            .publish_matching(&event, Query::matches_intent_event, |token, event| {
                frames.push((token, self.publisher.publish(token, event.clone())))
            });
        let mut delivered = 0;
        for (token, frame) in frames {
            match self.deliver(token, frame.encode()?) {
                Ok(()) => delivered += 1,
                Err(_error) => {
                    self.registry.unregister(token);
                    self.writers.remove(&token);
                }
            }
        }
        Ok(delivered)
    }

    fn deliver(
        &mut self,
        token: IntentSubscriptionToken,
        frame: Vec<u8>,
    ) -> Result<(), SubscriptionError> {
        let Some(writer) = self.writers.get_mut(&token) else {
            self.registry.unregister(token);
            return Ok(());
        };
        LengthPrefixedCodec::default().write_body(writer, &FrameBody::new(frame))?;
        writer.flush()?;
        Ok(())
    }

    fn len(&self) -> usize {
        self.registry.len()
    }
}

impl Query {
    pub fn matches_intent_event(&self, event: &IntentEvent) -> bool {
        match event {
            IntentEvent::IntentRecorded(recorded) => recorded.entry.matches(self),
        }
    }
}
