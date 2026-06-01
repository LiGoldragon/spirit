use crate::{
    DatabaseMarker, Input, MailLedger, MessageIdentifier, MessageProcessed, MessageProcessedHook,
    NexusEngine, NexusInput, NexusMail, NexusOutput, OriginRoute, Output, SemaEngine,
    schema::lib::{nexus as nexus_plane, sema as sema_plane, signal as signal_plane},
    store::Store,
};

/// Nexus — the runtime mail keeper (records 966/970).
///
/// Nexus owns the SEMA store handle and the mail ledger, and holds each
/// mail in a type-level BEING-PROCESSED state across the SEMA call. The
/// mail's Rust type reflects its lifecycle: [`Mail<BeingProcessed>`] is in
/// flight, and only running SEMA turns it into [`Mail<Processed>`]. "Nexus
/// holds the mail ⇒ it is being processed" is therefore a compile-time
/// fact, not a logging convention — there is no method that yields a
/// `Mail<Processed>` without consuming a `Mail<BeingProcessed>` through the
/// store.
///
/// Nexus is one of the daemon's three execution centers (Signal accepts /
/// validates, Nexus holds + translates + invokes SEMA + emits events, SEMA
/// is the durable store). It owns BOTH translations: Signal payload mail
/// into the SEMA language, and the SEMA reply back into the Signal output.
#[derive(Debug)]
pub struct Nexus {
    store: Store,
    mail_ledger: MailLedger,
}

/// A mail Nexus is holding, parameterised by its lifecycle phase.
///
/// The `phase` field carries the phase-specific data, so the two phases
/// are structurally different values, not one struct wearing a marker:
/// [`BeingProcessed`] carries the Nexus input the mail will run;
/// [`Processed`] carries the Signal output the SEMA reply produced. The
/// only transition is the private `run_nexus` (it consumes a
/// `Mail<BeingProcessed>` and threads it through the Nexus engine), so a
/// `Mail<Processed>` cannot exist until SEMA has run.
#[derive(Debug)]
pub struct Mail<Phase> {
    identifier: MessageIdentifier,
    origin_route: OriginRoute,
    phase: Phase,
}

/// In-flight phase: Nexus owns the mail, the SEMA call has not yet run.
#[derive(Debug)]
pub struct BeingProcessed {
    nexus_input: nexus_plane::Nexus<nexus_plane::Input>,
}

/// Reached phase: the SEMA reply, translated back into the Signal output.
#[derive(Debug)]
pub struct Processed {
    output: signal_plane::Signal<Output>,
}

impl Nexus {
    /// Build a Nexus over a durable SEMA store and a fresh mail ledger.
    pub fn new(store: Store) -> Self {
        Self {
            store,
            mail_ledger: MailLedger::default(),
        }
    }

    /// Hold the accepted mail across the SEMA call and reply.
    ///
    /// The mail enters [`BeingProcessed`] the moment Nexus owns it; the
    /// SEMA write/read runs WHILE Nexus holds it; the reply turns it into
    /// [`Processed`]; the processed lifecycle event is emitted; and the
    /// Signal output leaves. This is the literal record-970 flow.
    pub fn process<Payload>(&mut self, mail: NexusMail<Payload>) -> signal_plane::Signal<Output>
    where
        Mail<BeingProcessed>: FromMail<Payload>,
    {
        let in_flight = Mail::<BeingProcessed>::from_mail(mail);
        self.process_in_flight(in_flight).into_output()
    }

    pub fn process_nexus_input(
        &mut self,
        identifier: MessageIdentifier,
        input: nexus_plane::Nexus<nexus_plane::Input>,
    ) -> nexus_plane::Nexus<nexus_plane::Output> {
        let in_flight = Mail::<BeingProcessed>::from_nexus_input(identifier, input);
        let processed = self.process_in_flight(in_flight);
        processed.into_nexus_output()
    }

    fn process_in_flight(&mut self, in_flight: Mail<BeingProcessed>) -> Mail<Processed> {
        let processed = in_flight.run_nexus(self);
        processed
            .emit_processed(&mut self.mail_ledger.hook())
            .expect("spirit-next mail ledger is infallible");
        processed
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

/// Take ownership of accepted Signal payload mail, lowering it through the
/// generated Nexus projection into the SEMA language so the in-flight mail
/// already speaks SEMA.
pub trait FromMail<Payload> {
    fn from_mail(mail: NexusMail<Payload>) -> Self;
}

impl<Payload> FromMail<Payload> for Mail<BeingProcessed>
where
    Input: From<Payload>,
    NexusInput: From<Input>,
{
    fn from_mail(mail: NexusMail<Payload>) -> Self {
        let identifier = mail.identifier();
        let origin_route = mail.origin_route();
        Self {
            identifier,
            origin_route,
            phase: BeingProcessed {
                nexus_input: mail.into_nexus_input(),
            },
        }
    }
}

impl Mail<BeingProcessed> {
    pub fn from_nexus_input(
        identifier: MessageIdentifier,
        input: nexus_plane::Nexus<nexus_plane::Input>,
    ) -> Self {
        let origin_route = input.origin_route();
        Self {
            identifier,
            origin_route,
            phase: BeingProcessed { nexus_input: input },
        }
    }

    pub fn identifier(&self) -> MessageIdentifier {
        self.identifier
    }

    pub fn origin_route(&self) -> OriginRoute {
        self.origin_route
    }

    /// The SEMA-language form the in-flight mail will hand to the store.
    pub fn sema_input(&self) -> sema_plane::Sema<sema_plane::Input> {
        self.phase
            .nexus_input
            .clone()
            .into_nexus_output()
            .into_sema_input()
    }

    /// Run the Nexus engine while Nexus holds the mail, then transition the
    /// mail type to [`Processed`]. This is the ONLY constructor of a
    /// `Mail<Processed>`; the Nexus reply is translated back into the
    /// Signal output here.
    fn run_nexus(self, nexus: &mut Nexus) -> Mail<Processed> {
        let nexus_output = NexusEngine::execute(nexus, self.phase.nexus_input);
        let output = nexus_output.into_signal_output();
        Mail {
            identifier: self.identifier,
            origin_route: self.origin_route,
            phase: Processed { output },
        }
    }
}

impl Mail<Processed> {
    pub fn identifier(&self) -> MessageIdentifier {
        self.identifier
    }

    pub fn origin_route(&self) -> OriginRoute {
        self.origin_route
    }

    pub fn output(&self) -> &signal_plane::Signal<Output> {
        &self.phase.output
    }

    pub fn database_marker(&self) -> DatabaseMarker {
        self.phase.output.root().database_marker()
    }

    /// Push the [`MessageProcessed`] lifecycle event built from the
    /// reached mail through the ledger hook.
    fn emit_processed<Hook>(&self, hook: &mut Hook) -> Result<(), Hook::Error>
    where
        Hook: MessageProcessedHook<Output>,
    {
        MessageProcessed::new(
            self.identifier,
            self.origin_route,
            self.phase.output.root().clone(),
        )
        .push_to(hook)
    }

    fn into_output(self) -> signal_plane::Signal<Output> {
        self.phase.output
    }

    fn into_nexus_output(self) -> nexus_plane::Nexus<NexusOutput> {
        let origin_route = self.origin_route;
        NexusOutput::from(self.phase.output.into_root()).with_origin_route(origin_route)
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
            NexusOutput::Sema(input) => {
                let sema_output =
                    SemaEngine::apply(&mut self.store, input.with_origin_route(origin_route));
                sema_output.into_nexus_input().into_nexus_output()
            }
            NexusOutput::Signal(output) => {
                NexusOutput::from(output).with_origin_route(origin_route)
            }
        }
    }
}
