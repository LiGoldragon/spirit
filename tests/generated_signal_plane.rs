use signal_frame::{
    ExchangeLane, LaneSequence, SessionEpoch, StreamEventIdentifier, StreamingFrameBody,
    SubscriptionTokenInner,
};
use spirit::schema::signal::{
    CertaintyChange, CertaintyChangeReceipt, DatabaseMarker, Entry, Input, InputRoute, IntentEvent,
    IntentRecorded, Kind, Magnitude, MessageIdentifier, MessageRoot, OriginRoute, Output,
    OutputRoute, Record, RecordChange, RecordChangeReceipt, RecordSelection, Rejected, SemaReceipt,
    SignalFrameError, SignalRejection, Statement, TopicMatch, ValidationError,
};

fn marker(commit_sequence: u64, state_digest: u64) -> DatabaseMarker {
    DatabaseMarker {
        commit_sequence,
        state_digest,
    }
}

#[test]
fn generated_input_surface_owns_route_header_and_rkyv_frame() {
    let input = Input::record(Entry {
        topics: vec![String::from("schema")],
        kind: Kind::Constraint,
        description: String::from("schema creates the signal plane"),
        magnitude: Magnitude::Maximum,
        privacy: Magnitude::Zero,
    });

    assert_eq!(input.route(), InputRoute::Record);

    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, InputRoute::Record);
    assert_eq!(decoded, input);
}

#[test]
fn generated_output_surface_owns_route_header_and_rkyv_frame() {
    let output = Output::record_accepted(SemaReceipt {
        record_identifier: String::from("003g"),
        database_marker: marker(3, 97),
    });

    assert_eq!(output.route(), OutputRoute::RecordAccepted);

    let frame = output.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Output::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, OutputRoute::RecordAccepted);
    assert_eq!(decoded, output);
}

#[test]
fn generated_state_input_surface_owns_route_header_and_rkyv_frame() {
    let input = Input::state(Statement::new(String::from("capture this intent")));

    assert_eq!(input.route(), InputRoute::State);

    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, InputRoute::State);
    assert_eq!(decoded, input);
}

#[test]
fn generated_public_private_record_shortcut_roots_own_route_header_and_rkyv_frame() {
    let public_input = Input::public_records(RecordSelection {
        topic_match: TopicMatch::full(vec![String::from("schema")]),
        kind: Some(Kind::Decision),
    });
    let private_input = Input::private_records(RecordSelection {
        topic_match: TopicMatch::partial(vec![String::from("schema")]),
        kind: None,
    });

    assert_eq!(public_input.route(), InputRoute::PublicRecords);
    assert_eq!(private_input.route(), InputRoute::PrivateRecords);

    let public_frame = public_input.encode_signal_frame().expect("encode frame");
    let private_frame = private_input.encode_signal_frame().expect("encode frame");
    let (public_route, public_decoded) =
        Input::decode_signal_frame(&public_frame).expect("decode public frame");
    let (private_route, private_decoded) =
        Input::decode_signal_frame(&private_frame).expect("decode private frame");

    assert_eq!(public_route, InputRoute::PublicRecords);
    assert_eq!(private_route, InputRoute::PrivateRecords);
    assert_eq!(public_decoded, public_input);
    assert_eq!(private_decoded, private_input);
}

#[test]
fn generated_change_certainty_surface_owns_route_header_and_rkyv_frame() {
    let input = Input::change_certainty(CertaintyChange {
        record_identifier: String::from("003g"),
        certainty: Magnitude::Zero,
    });

    assert_eq!(input.route(), InputRoute::ChangeCertainty);

    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, InputRoute::ChangeCertainty);
    assert_eq!(decoded, input);
}

#[test]
fn generated_certainty_changed_output_owns_route_header_and_rkyv_frame() {
    let output = Output::certainty_changed(CertaintyChangeReceipt {
        record_identifier: String::from("003g"),
        certainty: Magnitude::Zero,
        database_marker: marker(4, 101),
    });

    assert_eq!(output.route(), OutputRoute::CertaintyChanged);

    let frame = output.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Output::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, OutputRoute::CertaintyChanged);
    assert_eq!(decoded, output);
}

#[test]
fn generated_record_change_surface_owns_route_header_and_rkyv_frame() {
    let replacement = Entry {
        topics: vec![String::from("schema"), String::from("mutation")],
        kind: Kind::Correction,
        description: String::from("record mutation is a schema-visible operation"),
        magnitude: Magnitude::High,
        privacy: Magnitude::Zero,
    };
    let input = Input::change_record(RecordChange {
        record_identifier: String::from("003g"),
        entry: replacement,
    });

    assert_eq!(input.route(), InputRoute::ChangeRecord);

    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, InputRoute::ChangeRecord);
    assert_eq!(decoded, input);
}

#[test]
fn generated_record_changed_output_owns_route_header_and_rkyv_frame() {
    let output = Output::record_changed(RecordChangeReceipt {
        record_identifier: String::from("003g"),
        database_marker: marker(4, 101),
    });

    assert_eq!(output.route(), OutputRoute::RecordChanged);

    let frame = output.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Output::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, OutputRoute::RecordChanged);
    assert_eq!(decoded, output);
}

#[test]
fn generated_streaming_surface_owns_subscription_event_frames() {
    let event = IntentEvent::intent_recorded(IntentRecorded {
        entry: Entry {
            topics: vec![String::from("stream")],
            kind: Kind::Decision,
            description: String::from("schema emits streaming frames"),
            magnitude: Magnitude::High,
            privacy: Magnitude::Zero,
        },
        sema_receipt: SemaReceipt {
            record_identifier: String::from("003g"),
            database_marker: marker(3, 97),
        },
    });

    let frame = event.clone().into_subscription_frame(
        StreamEventIdentifier::new(
            SessionEpoch::new(1),
            ExchangeLane::Acceptor,
            LaneSequence::first(),
        ),
        SubscriptionTokenInner::new(44),
    );
    let bytes = frame.encode_length_prefixed().expect("encode frame");
    let decoded = spirit::schema::signal::Frame::decode_length_prefixed(&bytes)
        .expect("decode streaming frame");

    match decoded.into_body() {
        StreamingFrameBody::SubscriptionEvent {
            event_identifier,
            token,
            event: decoded_event,
        } => {
            assert_eq!(event_identifier.sequence, LaneSequence::first());
            assert_eq!(token, SubscriptionTokenInner::new(44));
            assert_eq!(decoded_event, event);
        }
        other => panic!("expected generated subscription event frame, got {other:?}"),
    }
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_state_input_round_trips_the_canonical_newtype_shape() {
    let input = "(State [capture this intent])"
        .parse::<Input>()
        .expect("parse state input");

    assert_eq!(
        input,
        Input::state(Statement::new(String::from("capture this intent")))
    );
    assert_eq!(input.to_string(), "(State [capture this intent])");
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_change_certainty_round_trips_the_canonical_shape() {
    let input = "(ChangeCertainty ([003g] Zero))"
        .parse::<Input>()
        .expect("parse change certainty input");

    assert_eq!(
        input,
        Input::change_certainty(CertaintyChange {
            record_identifier: String::from("003g"),
            certainty: Magnitude::Zero,
        })
    );
    assert_eq!(input.to_string(), "(ChangeCertainty ([003g] Zero))");
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_change_record_round_trips_the_canonical_shape() {
    let input = "(ChangeRecord ([003g] ([[schema mutation]] Correction [replacement] High Zero)))"
        .parse::<Input>()
        .expect("parse change record input");

    assert_eq!(
        input,
        Input::change_record(RecordChange {
            record_identifier: String::from("003g"),
            entry: Entry {
                topics: vec![String::from("schema mutation")],
                kind: Kind::Correction,
                description: String::from("replacement"),
                magnitude: Magnitude::High,
                privacy: Magnitude::Zero,
            },
        })
    );
    assert_eq!(
        input.to_string(),
        "(ChangeRecord ([003g] ([[schema mutation]] Correction replacement High Zero)))"
    );
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_public_private_record_shortcuts_round_trip_nota() {
    let public_input = "(PublicRecords ((Full [schema]) (Some Decision)))"
        .parse::<Input>()
        .expect("parse public records input");
    let private_input = "(PrivateRecords ((Partial [schema]) None))"
        .parse::<Input>()
        .expect("parse private records input");

    assert_eq!(
        public_input,
        Input::public_records(RecordSelection {
            topic_match: TopicMatch::full(vec![String::from("schema")]),
            kind: Some(Kind::Decision),
        })
    );
    assert_eq!(
        private_input,
        Input::private_records(RecordSelection {
            topic_match: TopicMatch::partial(vec![String::from("schema")]),
            kind: None,
        })
    );
    assert_eq!(
        public_input.to_string(),
        "(PublicRecords ((Full [schema]) (Some Decision)))"
    );
    assert_eq!(
        private_input.to_string(),
        "(PrivateRecords ((Partial [schema]) None))"
    );
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_record_input_renders_bracket_bearing_strings_losslessly() {
    let description = String::from("text contains [brackets] and the pipe close marker |]");
    let input = Input::record(Entry {
        topics: vec![String::from("schema replay")],
        kind: Kind::Correction,
        description: description.clone(),
        magnitude: Magnitude::High,
        privacy: Magnitude::Zero,
    });
    let rendered = input.to_string();

    assert_eq!(
        rendered,
        "(Record ([[schema replay]] Correction [|text contains [brackets] and the pipe close marker \\|]|] High Zero))"
    );
    let reparsed = rendered
        .parse::<Input>()
        .expect("generated Input should parse its own bracket-safe NOTA");
    assert_eq!(reparsed, input);
}

#[test]
fn generated_rejection_output_is_a_signal_schema_variant() {
    let output = Output::rejected(SignalRejection {
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

#[test]
fn bare_schema_bindings_are_direct_payload_aliases_not_wrappers() {
    let record: Record = Entry {
        topics: vec![String::from("schema")],
        kind: Kind::Constraint,
        description: String::from("alias bindings carry direct payloads"),
        magnitude: Magnitude::Maximum,
        privacy: Magnitude::Zero,
    };
    let input = Input::Record(record);
    match input {
        Input::Record(entry) => {
            assert_eq!(entry.description, "alias bindings carry direct payloads");
        }
        other => panic!("expected direct Record payload, got {other:?}"),
    }

    let rejected: Rejected = SignalRejection {
        validation_error: ValidationError::EmptyTopic,
        database_marker: marker(0, 0),
    };
    let output = Output::Rejected(rejected);
    match output {
        Output::Rejected(rejection) => {
            assert_eq!(rejection.validation_error, ValidationError::EmptyTopic);
            assert_eq!(rejection.database_marker, marker(0, 0));
        }
        other => panic!("expected direct Rejected payload, got {other:?}"),
    }
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_validation_error_round_trips_through_nota() {
    let rejection = "(Rejected (EmptyDescription (0 0)))"
        .parse::<Output>()
        .expect("parse rejection output");

    assert_eq!(
        rejection,
        Output::rejected(SignalRejection {
            validation_error: ValidationError::EmptyDescription,
            database_marker: marker(0, 0),
        })
    );
    assert_eq!(rejection.to_string(), "(Rejected (EmptyDescription (0 0)))");
}

#[test]
fn generated_signal_surface_rejects_unknown_header_before_body_decode() {
    let mut frame = Input::record(Entry {
        topics: vec![String::from("schema")],
        kind: Kind::Constraint,
        description: String::from("schema rejects unknown routes"),
        magnitude: Magnitude::Maximum,
        privacy: Magnitude::Zero,
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
    let input = Input::record(Entry {
        topics: vec![String::from("schema")],
        kind: Kind::Constraint,
        description: String::from("schema emits mail events"),
        magnitude: Magnitude::Maximum,
        privacy: Magnitude::Zero,
    });

    let message = input.with_origin_route(OriginRoute(91));
    let event = message.message_sent(MessageIdentifier(9));

    assert_eq!(event.identifier, MessageIdentifier(9));
    assert_eq!(event.origin_route(), OriginRoute(91));
    assert_ne!(event.origin_route(), OriginRoute(event.identifier.0));
    assert_eq!(event.root, MessageRoot::Input);
    assert_eq!(event.short_header, message.root().short_header());
}
