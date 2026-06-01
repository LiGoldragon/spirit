use crate::{
    DatabaseMarker, MailLedger, NexusEngine, NexusOutput, SemaEngine,
    schema::lib::nexus as nexus_plane, store::Store,
};

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
}

impl Nexus {
    /// Build a Nexus over a durable SEMA store and a fresh mail ledger.
    pub fn new(store: Store) -> Self {
        Self {
            store,
            mail_ledger: MailLedger::default(),
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
