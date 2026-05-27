use spirit_next::{
    CommitSequence, DatabaseMarker, Description, Engine, Entry, ErrorMessage, ErrorReport, Input,
    Kind, Magnitude, MailIdentifier, MailLedgerEvent, MessageIdentifier, NexusMail, Output,
    ProcessedMail, Query, RecordIdentifier, RecordSet, SemaCommand, SemaReceipt, SemaResponse,
    SentMail, ShortHeader, StateDigest, Store, Topic,
};

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

#[test]
fn nexus_mail_lowers_signal_payload_to_generated_sema_command() {
    let command = NexusMail::new(MessageIdentifier(1), entry("nexus mail lowers to SEMA"))
        .into_sema_command();

    match command {
        SemaCommand::Record(recorded) => {
            assert_eq!(recorded.description.0, "nexus mail lowers to SEMA");
        }
        SemaCommand::Observe(_) => panic!("record input should lower to record command"),
    }
}

#[test]
fn sema_store_is_the_single_writer_for_records() {
    let mut store = Store::default();

    let response = store.apply(SemaCommand::Record(entry("SEMA writes durable facts")));

    assert_eq!(
        response,
        SemaResponse::Recorded(SemaReceipt {
            record_identifier: RecordIdentifier(1),
            database_marker: marker(1, 39),
        })
    );
    assert_eq!(store.len(), 1);
}

#[test]
fn sema_response_maps_back_to_signal_output() {
    let output = SemaResponse::Missed(ErrorReport {
        error_message: ErrorMessage(String::from("no matching record")),
        database_marker: marker(0, 0),
    })
    .into_output();

    assert_eq!(
        output,
        Output::Error(ErrorReport {
            error_message: ErrorMessage(String::from("no matching record")),
            database_marker: marker(0, 0),
        })
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
