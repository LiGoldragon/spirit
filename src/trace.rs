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
    SignalAdmission {
        origin_route: OriginRoute,
        input: Input,
    },
    SignalRejection {
        origin_route: OriginRoute,
        validation_error: ValidationError,
    },
    SignalReply {
        origin_route: OriginRoute,
        output: Output,
    },
    NexusExecute {
        origin_route: OriginRoute,
        input: NexusInput,
    },
    NexusDecision {
        origin_route: OriginRoute,
        output: NexusOutput,
    },
    SemaApply {
        origin_route: OriginRoute,
        input: SemaWriteInput,
        output: SemaWriteOutput,
    },
    SemaObserve {
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
