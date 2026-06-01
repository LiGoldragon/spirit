use std::sync::{Arc, Mutex};

use crate::{
    Input, NexusInput, NexusOutput, OriginRoute, Output, SemaReadInput, SemaReadOutput,
    SemaWriteInput, SemaWriteOutput, ValidationError,
};

#[derive(Clone, Debug, Default)]
pub struct TraceLog {
    events: Option<Arc<Mutex<Vec<TraceEvent>>>>,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    SignalAdmitted {
        origin_route: OriginRoute,
        input: Input,
    },
    SignalRejected {
        origin_route: OriginRoute,
        validation_error: ValidationError,
    },
    SignalReplied {
        origin_route: OriginRoute,
        output: Output,
    },
    NexusEntered {
        origin_route: OriginRoute,
        input: NexusInput,
    },
    NexusDecided {
        origin_route: OriginRoute,
        output: NexusOutput,
    },
    SemaWriteApplied {
        origin_route: OriginRoute,
        input: SemaWriteInput,
        output: SemaWriteOutput,
    },
    SemaReadObserved {
        origin_route: OriginRoute,
        input: SemaReadInput,
        output: SemaReadOutput,
    },
}

impl TraceLog {
    pub fn disabled() -> Self {
        Self { events: None }
    }

    pub fn recording() -> Self {
        Self {
            events: Some(Arc::new(Mutex::new(Vec::new()))),
        }
    }

    pub fn events(&self) -> Vec<TraceEvent> {
        self.events
            .as_ref()
            .map(|events| events.lock().expect("trace event lock").clone())
            .unwrap_or_default()
    }

    pub fn record(&self, event: TraceEvent) {
        if let Some(events) = &self.events {
            events.lock().expect("trace event lock").push(event);
        }
    }
}
