use std::{cell::RefCell, convert::Infallible};

use spirit_next::{
    CommitSequence, DatabaseMarker, Description, Engine, Entry, ErrorMessage, ErrorReport, Export,
    Import, Input, InputNexus, Kind, LocalPath, Magnitude, MailIdentifier, MailLedgerEvent,
    MessageIdentifier, MessageSent, MessageSentHook, NexusInput, NexusMail, NexusOutput, Output,
    ProcessedMail, PublicPath, Query, RecordIdentifier, RecordSet, SemaInput, SemaOutput,
    SemaReceipt, SentMail, ShortHeader, SignalActor, SourcePath, StateDigest, Store, Topic,
};

#[derive(Default)]
struct SentHookProbe {
    events: Vec<MessageSent>,
}

#[derive(Default)]
struct NexusProbe {
    accepted_identifiers: RefCell<Vec<MessageIdentifier>>,
    trace: RefCell<Vec<TraceEvent>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TraceEvent {
    SentHook(MessageIdentifier),
    NexusAccepted(MessageIdentifier),
}

impl NexusProbe {
    fn accepted_identifiers(&self) -> Vec<MessageIdentifier> {
        self.accepted_identifiers.borrow().clone()
    }

    fn trace(&self) -> Vec<TraceEvent> {
        self.trace.borrow().clone()
    }
}

impl InputNexus for NexusProbe {
    type Reply = NexusOutput;
    type Error = Infallible;

    fn record(&self, mail: NexusMail<Entry>) -> Result<Self::Reply, Self::Error> {
        self.trace
            .borrow_mut()
            .push(TraceEvent::NexusAccepted(mail.identifier()));
        self.accepted_identifiers
            .borrow_mut()
            .push(mail.identifier());
        Ok(mail.into_nexus_input().into_nexus_output())
    }

    fn observe(&self, mail: NexusMail<Query>) -> Result<Self::Reply, Self::Error> {
        self.trace
            .borrow_mut()
            .push(TraceEvent::NexusAccepted(mail.identifier()));
        self.accepted_identifiers
            .borrow_mut()
            .push(mail.identifier());
        Ok(mail.into_nexus_input().into_nexus_output())
    }
}

fn entry(description: &str) -> Entry {
    Entry {
        topic: Topic(String::from("runtime-triad")),
        kind: Kind::Decision,
        description: Description(String::from(description)),
        magnitude: Magnitude::Maximum,
    }
}

fn marker(commit_sequence: u64, state_digest: u64) -> DatabaseMarker {
    DatabaseMarker {
        commit_sequence: CommitSequence(commit_sequence),
        state_digest: StateDigest(state_digest),
    }
}

impl SentHookProbe {
    fn record_into_trace(self, nexus: &NexusProbe) -> SentHookTrace<'_> {
        SentHookTrace {
            events: self.events,
            trace: &nexus.trace,
        }
    }
}

struct SentHookTrace<'a> {
    events: Vec<MessageSent>,
    trace: &'a RefCell<Vec<TraceEvent>>,
}

impl MessageSentHook for SentHookTrace<'_> {
    type Error = Infallible;

    fn message_sent(&mut self, event: MessageSent) -> Result<(), Self::Error> {
        self.trace
            .borrow_mut()
            .push(TraceEvent::SentHook(event.identifier));
        self.events.push(event);
        Ok(())
    }
}

#[test]
fn nexus_mail_lowers_signal_payload_to_generated_sema_command() {
    let command = NexusMail::new(MessageIdentifier(1), entry("nexus mail lowers to SEMA"))
        .into_nexus_input()
        .into_nexus_output()
        .into_sema_input();

    match command {
        SemaInput::Record(recorded) => {
            assert_eq!(recorded.description.0, "nexus mail lowers to SEMA");
        }
        SemaInput::Observe(_) => panic!("record input should lower to record command"),
    }
}

#[test]
fn signal_actor_pushes_accepted_message_through_sent_hook_to_nexus() {
    let signal_actor = SignalActor::default();
    let accepted = signal_actor.accept(Input::Record(entry("signal pushes to nexus")));
    let nexus = NexusProbe::default();
    let mut hook = SentHookProbe::default().record_into_trace(&nexus);

    assert_eq!(accepted.message_sent().identifier, MessageIdentifier(1));
    assert_eq!(hook.events, []);
    assert_eq!(nexus.trace(), []);

    let processed = accepted
        .push_to_nexus(&nexus, &mut hook)
        .expect("signal to nexus push");

    assert_eq!(hook.events.len(), 1);
    assert_eq!(hook.events[0].identifier, MessageIdentifier(1));
    assert_eq!(nexus.accepted_identifiers(), vec![MessageIdentifier(1)]);
    assert_eq!(
        nexus.trace(),
        vec![
            TraceEvent::SentHook(MessageIdentifier(1)),
            TraceEvent::NexusAccepted(MessageIdentifier(1)),
        ]
    );
    assert!(matches!(
        processed.into_reply(),
        NexusOutput::Sema(SemaInput::Record(_))
    ));
}

#[test]
fn sema_store_is_the_single_writer_for_records() {
    let mut store = Store::default();

    let response = store.apply(SemaInput::Record(entry("SEMA writes durable facts")));

    assert_eq!(
        response,
        SemaOutput::Recorded(SemaReceipt {
            record_identifier: RecordIdentifier(1),
            database_marker: marker(1, 39),
        })
    );
    assert_eq!(store.len(), 1);
}

#[test]
fn sema_response_maps_back_to_signal_output() {
    let output = NexusInput::Sema(SemaOutput::Missed(ErrorReport {
        error_message: ErrorMessage(String::from("no matching record")),
        database_marker: marker(0, 0),
    }))
    .into_nexus_output()
    .into_signal_output();

    assert_eq!(
        output,
        Output::Error(ErrorReport {
            error_message: ErrorMessage(String::from("no matching record")),
            database_marker: marker(0, 0),
        })
    );
}

#[test]
fn nexus_and_sema_have_explicit_input_output_languages() {
    let nexus_input = NexusInput::Signal(Input::Record(entry("language input")));
    let nexus_output = nexus_input.into_nexus_output();
    assert!(matches!(
        nexus_output,
        NexusOutput::Sema(SemaInput::Record(_))
    ));

    let sema_output = SemaOutput::Recorded(SemaReceipt {
        record_identifier: RecordIdentifier(3),
        database_marker: marker(4, 127),
    });
    let signal_output = NexusInput::Sema(sema_output)
        .into_nexus_output()
        .into_signal_output();
    assert!(matches!(signal_output, Output::RecordAccepted(_)));
}

#[test]
fn import_export_paths_use_single_colon_namespaces() {
    let import = Import {
        source_path: SourcePath(String::from("signal:sema:Magnitude")),
        local_path: LocalPath(String::from("spirit:core:Magnitude")),
    };
    let export = Export {
        local_path: LocalPath(String::from("spirit:core:SemaOutput")),
        public_path: PublicPath(String::from("spirit:sema:SemaOutput")),
    };

    assert_eq!(
        import.to_nota(),
        "([signal:sema:Magnitude] [spirit:core:Magnitude])"
    );
    assert_eq!(
        export.to_nota(),
        "([spirit:core:SemaOutput] [spirit:sema:SemaOutput])"
    );
}

#[test]
fn full_runtime_triad_records_then_observes() {
    let engine = Engine::default();

    let recorded = engine.handle(Input::Record(entry("full runtime triad works")));
    assert_eq!(
        recorded,
        Output::RecordAccepted(SemaReceipt {
            record_identifier: RecordIdentifier(1),
            database_marker: marker(1, 39),
        })
    );
    assert_eq!(engine.sent_message_count(), 1);
    assert_eq!(engine.processed_message_count(), 1);

    let observed = engine.handle(Input::Observe(Query {
        topic: Topic(String::from("runtime-triad")),
        kind: Kind::Decision,
    }));

    assert_eq!(
        observed,
        Output::RecordsObserved(spirit_next::ObservedRecords {
            record_set: RecordSet(entry("full runtime triad works")),
            database_marker: marker(1, 39),
        })
    );
    assert_eq!(engine.sent_message_count(), 2);
    assert_eq!(engine.processed_message_count(), 2);

    assert_eq!(
        engine.mail_ledger(),
        vec![
            MailLedgerEvent::Sent(SentMail {
                mail_identifier: MailIdentifier(1),
                short_header: ShortHeader(0),
            }),
            MailLedgerEvent::Processed(ProcessedMail {
                mail_identifier: MailIdentifier(1),
                database_marker: marker(1, 39),
            }),
            MailLedgerEvent::Sent(SentMail {
                mail_identifier: MailIdentifier(2),
                short_header: ShortHeader(0x0001_0000_0000_0000),
            }),
            MailLedgerEvent::Processed(ProcessedMail {
                mail_identifier: MailIdentifier(2),
                database_marker: marker(1, 39),
            }),
        ]
    );
}
