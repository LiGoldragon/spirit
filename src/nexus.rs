use crate::{
    DatabaseMarker, MailLedger, NexusEngine, NexusOutput, SemaEngine,
    schema::lib::nexus as nexus_plane, store::Store,
};

#[cfg(feature = "testing-trace")]
use crate::{TraceEvent, TraceLog, TraceObject};

/// Nexus is the runtime decision center between Signal and SEMA.
///
/// Signal admits and triages wire input. Nexus owns the durable SEMA
/// handle, translates Signal work into write/read SEMA operations, waits
/// for the SEMA reply, and translates the result back toward Signal. The
/// `NexusEngine::execute(&mut self, ...)` borrow is the single-flight
/// guard: one Nexus cannot execute two messages at the same time.
#[derive(Debug)]
pub struct Nexus {
    store: Store,
    mail_ledger: MailLedger,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
}

impl Nexus {
    /// Build a Nexus over a durable SEMA store and a fresh mail ledger.
    pub fn new(store: Store) -> Self {
        #[cfg(feature = "testing-trace")]
        {
            Self::new_with_trace(store, TraceLog::default())
        }
        #[cfg(not(feature = "testing-trace"))]
        {
            Self {
                store,
                mail_ledger: MailLedger::default(),
            }
        }
    }

    #[cfg(feature = "testing-trace")]
    pub fn new_with_trace(store: Store, trace_log: TraceLog) -> Self {
        Self {
            store: store.with_trace(trace_log.clone()),
            mail_ledger: MailLedger::default(),
            trace_log,
        }
    }

    pub fn mail_ledger(&self) -> &MailLedger {
        &self.mail_ledger
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn database_marker(&self) -> DatabaseMarker {
        self.store.database_marker()
    }
}

impl NexusEngine for Nexus {
    #[cfg(feature = "testing-trace")]
    fn trace_nexus_activation(&self, object: TraceObject) {
        self.trace_log.record(TraceEvent::new(object));
    }

    fn decide(
        &mut self,
        input: nexus_plane::Nexus<nexus_plane::Input>,
    ) -> nexus_plane::Nexus<nexus_plane::Output> {
        let output = input.into_nexus_output();
        let origin_route = output.origin_route();
        match output.into_root() {
            NexusOutput::SemaWrite(input) => {
                let sema_output =
                    SemaEngine::apply(&mut self.store, input.with_origin_route(origin_route));
                sema_output.into_nexus_input().into_nexus_output()
            }
            NexusOutput::SemaRead(input) => {
                let sema_output =
                    SemaEngine::observe(&self.store, input.with_origin_route(origin_route));
                sema_output.into_nexus_input().into_nexus_output()
            }
            NexusOutput::Signal(output) => {
                NexusOutput::from(output).with_origin_route(origin_route)
            }
        }
    }
}
