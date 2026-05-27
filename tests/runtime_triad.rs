use spirit_next::{
    Description, Engine, Entry, ErrorMessage, Input, Kind, Magnitude, MessageIdentifier, NexusMail,
    Output, Query, RecordIdentifier, RecordSet, SemaCommand, SemaResponse, Store, Topic,
};

fn entry(description: &str) -> Entry {
    Entry {
        topic: Topic(String::from("runtime-triad")),
        kind: Kind::Decision,
        description: Description(String::from(description)),
        magnitude: Magnitude::Maximum,
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

    assert_eq!(response, SemaResponse::Recorded(RecordIdentifier(1)));
    assert_eq!(store.len(), 1);
}

#[test]
fn sema_response_maps_back_to_signal_output() {
    let output =
        SemaResponse::Missed(ErrorMessage(String::from("no matching record"))).into_output();

    assert_eq!(
        output,
        Output::Error(ErrorMessage(String::from("no matching record")))
    );
}

#[test]
fn full_runtime_triad_records_then_observes() {
    let engine = Engine::default();

    let recorded = engine.handle(Input::Record(entry("full runtime triad works")));
    assert_eq!(recorded, Output::RecordAccepted(RecordIdentifier(1)));
    assert_eq!(engine.sent_message_count(), 1);
    assert_eq!(engine.processed_message_count(), 1);

    let observed = engine.handle(Input::Observe(Query {
        topic: Topic(String::from("runtime-triad")),
        kind: Kind::Decision,
    }));

    assert_eq!(
        observed,
        Output::RecordsObserved(RecordSet(entry("full runtime triad works")))
    );
    assert_eq!(engine.sent_message_count(), 2);
    assert_eq!(engine.processed_message_count(), 2);
}
