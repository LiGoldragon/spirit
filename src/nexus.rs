use crate::{
    DatabaseMarker, MailLedger, NexusEngine, NexusOutput, SemaEngine,
    schema::lib::nexus as nexus_plane, store::Store,
};

#[cfg(feature = "testing-trace")]
use crate::{TraceEvent, TraceLog};

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
        Self {
            store,
            mail_ledger: MailLedger::default(),
            #[cfg(feature = "testing-trace")]
            trace_log: TraceLog::disabled(),
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
    fn execute(
        &mut self,
        input: nexus_plane::Nexus<nexus_plane::Input>,
    ) -> nexus_plane::Nexus<nexus_plane::Output> {
        #[cfg(feature = "testing-trace")]
        self.trace_log.record(TraceEvent::NexusEntered {
            origin_route: input.origin_route(),
            input: input.root().clone(),
        });
        let output = input.into_nexus_output();
        #[cfg(feature = "testing-trace")]
        self.trace_log.record(TraceEvent::NexusDecided {
            origin_route: output.origin_route(),
            output: output.root().clone(),
        });
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
