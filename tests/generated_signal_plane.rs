use spirit_next::{
    CommitSequence, DatabaseMarker, Description, Entry, Input, InputRoute, Kind, Magnitude,
    MessageIdentifier, MessageRoot, OriginRoute, Output, OutputRoute, RecordIdentifier,
    SemaReceipt, SignalFrameError, SignalRejection, StateDigest, Topic, Topics, ValidationError,
};

fn marker(commit_sequence: u64, state_digest: u64) -> DatabaseMarker {
    DatabaseMarker {
        commit_sequence: CommitSequence(commit_sequence),
        state_digest: StateDigest(state_digest),
    }
}

#[test]
fn generated_input_surface_owns_route_header_and_rkyv_frame() {
    let input = Input::Record(Entry {
        topics: Topics(vec![Topic(String::from("schema"))]),
        kind: Kind::Constraint,
        description: Description(String::from("schema creates the signal plane")),
        magnitude: Magnitude::Maximum,
    });

    assert_eq!(input.route(), InputRoute::Record);

    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, InputRoute::Record);
    assert_eq!(decoded, input);
}

#[test]
fn generated_output_surface_owns_route_header_and_rkyv_frame() {
    let output = Output::RecordAccepted(SemaReceipt {
        record_identifier: RecordIdentifier(7),
        database_marker: marker(3, 97),
    });

    assert_eq!(output.route(), OutputRoute::RecordAccepted);

    let frame = output.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Output::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, OutputRoute::RecordAccepted);
    assert_eq!(decoded, output);
}

#[test]
fn generated_rejection_output_is_a_signal_schema_variant() {
    let output = Output::Rejected(SignalRejection {
        validation_error: ValidationError::EmptyTopic,
        database_marker: marker(0, 0),
    });

    assert_eq!(output.route(), OutputRoute::Rejected);
    #[cfg(feature = "nota-text")]
    assert_eq!(output.to_string(), "(Rejected (EmptyTopic (0 0)))");

    let frame = output.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Output::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, OutputRoute::Rejected);
    assert_eq!(decoded, output);
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_validation_error_round_trips_through_nota() {
    let rejection = "(Rejected (EmptyDescription (0 0)))"
        .parse::<Output>()
        .expect("parse rejection output");

    assert_eq!(
        rejection,
        Output::Rejected(SignalRejection {
            validation_error: ValidationError::EmptyDescription,
            database_marker: marker(0, 0),
        })
    );
    assert_eq!(rejection.to_string(), "(Rejected (EmptyDescription (0 0)))");
}

#[test]
fn generated_signal_surface_rejects_unknown_header_before_body_decode() {
    let mut frame = Input::Record(Entry {
        topics: Topics(vec![Topic(String::from("schema"))]),
        kind: Kind::Constraint,
        description: Description(String::from("schema rejects unknown routes")),
        magnitude: Magnitude::Maximum,
    })
    .encode_signal_frame()
    .expect("encode frame");

    frame[..8].copy_from_slice(&0xFFFF_FFFF_FFFF_FFFF_u64.to_le_bytes());

    let error = Input::decode_signal_frame(&frame).expect_err("unknown route should fail");
    assert_eq!(
        error,
        SignalFrameError::UnknownHeader {
            root_enum: "Input",
            header: 0xFFFF_FFFF_FFFF_FFFF
        }
    );
}

#[test]
fn generated_signal_surface_emits_mail_sent_event() {
    let input = Input::Record(Entry {
        topics: Topics(vec![Topic(String::from("schema"))]),
        kind: Kind::Constraint,
        description: Description(String::from("schema emits mail events")),
        magnitude: Magnitude::Maximum,
    });

    let message = input.with_origin_route(OriginRoute(91));
    let event = message.message_sent(MessageIdentifier(9));

    assert_eq!(event.identifier, MessageIdentifier(9));
    assert_eq!(event.origin_route(), OriginRoute(91));
    assert_ne!(event.origin_route(), OriginRoute(event.identifier.0));
    assert_eq!(event.root, MessageRoot::Input);
    assert_eq!(event.short_header, message.root().short_header());
}
