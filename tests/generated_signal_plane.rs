mod support;

use signal_frame::{
    ExchangeLane, LaneSequence, SessionEpoch, StreamEventIdentifier, StreamingFrameBody,
    SubscriptionTokenInner,
};
use spirit::schema::signal::{
    Certainty, CertaintyChange, CertaintyChangeReceipt, DatabaseMarker, Description, DomainMatch,
    Entry, Input, InputRoute, IntentEvent, IntentRecordedEvent, Justification, Kind, Magnitude,
    Output, OutputRoute, Privacy, QuoteText, Reasoning, RecordChange, RecordChangeReceipt,
    RecordIdentifier, RecordInput, RecordRequest, RecordSelection, RejectedOutput, SearchText,
    SelectedKind, SignalFrameError, SignalRejection, Statement, StatementText, Testimony,
    ValidationError, VerbatimQuote, VersionReport, VersionText,
};
use spirit::{OriginRoute, SignalAdmission};
use support::domain_fixtures;

fn marker(commit_sequence: u64, state_digest: u64) -> DatabaseMarker {
    DatabaseMarker {
        commit_sequence: commit_sequence.into(),
        state_digest: state_digest.into(),
    }
}

fn record_request(entry: Entry, statement: &str) -> RecordRequest {
    RecordRequest {
        entry,
        justification: Justification {
            testimony: Testimony::new(vec![VerbatimQuote::new(
                QuoteText::new(statement.to_owned()),
                None,
            )]),
            reasoning: Reasoning::new(statement.to_owned()),
        },
    }
}

fn justification(statement: &str) -> Justification {
    Justification {
        testimony: Testimony::new(vec![VerbatimQuote::new(
            QuoteText::new(statement.to_owned()),
            None,
        )]),
        reasoning: Reasoning::new(statement.to_owned()),
    }
}

#[test]
fn generated_input_surface_owns_route_header_and_rkyv_frame() {
    let entry = Entry {
        domains: domain_fixtures::domains(&["schema"]),
        kind: Kind::Constraint,
        description: Description::new("schema creates the signal plane"),
        certainty: Magnitude::Maximum.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: spirit::schema::signal::Referents::new(vec![
            spirit::schema::signal::Referent::new("spirit"),
        ]),
    };
    let input = Input::record(record_request(entry, "schema creates the signal plane"));

    assert_eq!(input.route(), InputRoute::Record);

    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, InputRoute::Record);
    assert_eq!(decoded, input);
}

#[test]
fn generated_output_surface_owns_route_header_and_rkyv_frame() {
    let output = Output::record_accepted(RecordIdentifier::new("003g"));

    assert_eq!(output.route(), OutputRoute::RecordAccepted);

    let frame = output.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Output::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, OutputRoute::RecordAccepted);
    assert_eq!(decoded, output);
}

#[test]
fn generated_version_surface_owns_route_header_and_rkyv_frame() {
    let input = Input::Version;
    let output = Output::version_reported(VersionReport::new(VersionText::new(env!(
        "CARGO_PKG_VERSION"
    ))));

    assert_eq!(input.route(), InputRoute::Version);
    assert_eq!(output.route(), OutputRoute::VersionReported);

    let input_frame = input.encode_signal_frame().expect("encode input frame");
    let output_frame = output.encode_signal_frame().expect("encode output frame");
    let (input_route, decoded_input) =
        Input::decode_signal_frame(&input_frame).expect("decode input frame");
    let (output_route, decoded_output) =
        Output::decode_signal_frame(&output_frame).expect("decode output frame");

    assert_eq!(input_route, InputRoute::Version);
    assert_eq!(output_route, OutputRoute::VersionReported);
    assert_eq!(decoded_input, input);
    assert_eq!(decoded_output, output);
}

#[test]
fn generated_marker_surface_owns_route_header_and_rkyv_frame() {
    let input = Input::Marker;
    let output = Output::marker_reported(marker(4, 101));

    assert_eq!(input.route(), InputRoute::Marker);
    assert_eq!(output.route(), OutputRoute::MarkerReported);

    let input_frame = input.encode_signal_frame().expect("encode input frame");
    let output_frame = output.encode_signal_frame().expect("encode output frame");
    let (input_route, decoded_input) =
        Input::decode_signal_frame(&input_frame).expect("decode input frame");
    let (output_route, decoded_output) =
        Output::decode_signal_frame(&output_frame).expect("decode output frame");

    assert_eq!(input_route, InputRoute::Marker);
    assert_eq!(output_route, OutputRoute::MarkerReported);
    assert_eq!(decoded_input, input);
    assert_eq!(decoded_output, output);
}

#[test]
fn generated_state_input_surface_owns_route_header_and_rkyv_frame() {
    let input = Input::state(Statement::new(StatementText::new("capture this intent")));

    assert_eq!(input.route(), InputRoute::State);

    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, InputRoute::State);
    assert_eq!(decoded, input);
}

#[test]
fn generated_public_private_record_shortcut_roots_own_route_header_and_rkyv_frame() {
    let public_input = Input::public_records(RecordSelection {
        domain_match: DomainMatch::full(domain_fixtures::scopes(&["schema"])),
        selected_kind: SelectedKind::new(Some(Kind::Decision)),
    });
    let private_input = Input::private_records(RecordSelection {
        domain_match: DomainMatch::partial(domain_fixtures::scopes(&["schema"])),
        selected_kind: SelectedKind::new(None),
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
fn generated_public_text_search_root_owns_route_header_and_rkyv_frame() {
    let input = Input::public_text_search(SearchText::new("routing protocol"));

    assert_eq!(input.route(), InputRoute::PublicTextSearch);

    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, InputRoute::PublicTextSearch);
    assert_eq!(decoded, input);
}

#[test]
fn generated_public_intent_root_owns_route_header_and_rkyv_frame() {
    let input = Input::public_intent(spirit::schema::signal::DomainScopes::new(vec![
        spirit::schema::signal::DomainScope::All,
    ]));

    assert_eq!(input.route(), InputRoute::PublicIntent);

    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, InputRoute::PublicIntent);
    assert_eq!(decoded, input);
}

#[test]
fn generated_change_certainty_surface_owns_route_header_and_rkyv_frame() {
    let input = Input::change_certainty(CertaintyChange {
        record_identifier: RecordIdentifier::new("003g"),
        certainty: Certainty::new(Magnitude::Zero),
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
        record_identifier: RecordIdentifier::new("003g"),
        certainty: Certainty::new(Magnitude::Zero),
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
        domains: domain_fixtures::domains(&["schema", "mutation"]),
        kind: Kind::Correction,
        description: Description::new("record mutation is a schema-visible operation"),
        certainty: Magnitude::High.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: spirit::schema::signal::Referents::new(vec![
            spirit::schema::signal::Referent::new("spirit"),
        ]),
    };
    let input = Input::change_record(RecordChange {
        record_identifier: RecordIdentifier::new("003g"),
        entry: replacement,
        justification: justification("record mutation is a schema-visible operation"),
    });

    assert_eq!(input.route(), InputRoute::ChangeRecord);

    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, InputRoute::ChangeRecord);
    assert_eq!(decoded, input);
}

#[test]
fn generated_record_changed_output_owns_route_header_and_rkyv_frame() {
    let output = Output::record_changed(RecordChangeReceipt::new(RecordIdentifier::new("003g")));

    assert_eq!(output.route(), OutputRoute::RecordChanged);

    let frame = output.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Output::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, OutputRoute::RecordChanged);
    assert_eq!(decoded, output);
}

#[test]
fn generated_streaming_surface_owns_subscription_event_frames() {
    let event = IntentEvent::intent_recorded(IntentRecordedEvent {
        entry: Entry {
            domains: domain_fixtures::domains(&["stream"]),
            kind: Kind::Decision,
            description: Description::new("schema emits streaming frames"),
            certainty: Magnitude::High.into(),
            importance: Magnitude::Minimum.into(),
            privacy: Privacy::new(Magnitude::Zero),
            referents: spirit::schema::signal::Referents::new(vec![
                spirit::schema::signal::Referent::new("spirit"),
            ]),
        },
        record_identifier: RecordIdentifier::new("003g"),
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
        Input::state(Statement::new(StatementText::new("capture this intent")))
    );
    assert_eq!(input.to_string(), "(State [capture this intent])");
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_version_round_trips_the_canonical_atom_shape() {
    let input = "Version".parse::<Input>().expect("parse version input");
    let output = Output::version_reported(VersionReport::new(VersionText::new(env!(
        "CARGO_PKG_VERSION"
    ))));

    assert_eq!(input, Input::Version);
    assert_eq!(input.to_string(), "Version");
    assert_eq!(
        output.to_string(),
        format!("(VersionReported {})", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        output
            .to_string()
            .parse::<Output>()
            .expect("parse version output"),
        output
    );
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_change_certainty_round_trips_the_canonical_shape() {
    let input = "(ChangeCertainty (003g Zero))"
        .parse::<Input>()
        .expect("parse change certainty input");

    assert_eq!(
        input,
        Input::change_certainty(CertaintyChange {
            record_identifier: RecordIdentifier::new("003g"),
            certainty: Certainty::new(Magnitude::Zero),
        })
    );
    assert_eq!(input.to_string(), "(ChangeCertainty (003g Zero))");
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_change_record_round_trips_the_canonical_shape() {
    let input = "(ChangeRecord (003g ([(Technology (Software (Data SchemaEvolution)))] Correction replacement High Minimum Zero [spirit]) ([(replacement None)] replacement)))"
        .parse::<Input>()
        .expect("parse change record input");

    assert_eq!(
        input,
        Input::change_record(RecordChange {
            record_identifier: RecordIdentifier::new("003g"),
            entry: Entry {
                domains: domain_fixtures::domains(&["schema mutation"]),
                kind: Kind::Correction,
                description: Description::new("replacement"),
                certainty: Magnitude::High.into(),
                importance: Magnitude::Minimum.into(),
                privacy: Privacy::new(Magnitude::Zero),
                referents: spirit::schema::signal::Referents::new(vec![
                    spirit::schema::signal::Referent::new("spirit")
                ]),
            },
            justification: justification("replacement"),
        })
    );
    assert_eq!(
        input.to_string(),
        "(ChangeRecord (003g ([(Technology (Software (Data SchemaEvolution)))] Correction replacement High Minimum Zero [spirit]) ([(replacement None)] replacement)))"
    );
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_public_text_search_round_trips_nota() {
    let input = "(PublicTextSearch [routing protocol])"
        .parse::<Input>()
        .expect("parse public text search input");

    assert_eq!(
        input,
        Input::public_text_search(SearchText::new("routing protocol"))
    );
    assert_eq!(input.to_string(), "(PublicTextSearch [routing protocol])");
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_public_private_record_shortcuts_round_trip_nota() {
    let public_input =
        "(PublicRecords ((Full [(Technology (Software (Data SchemaEvolution)))]) (Some Decision)))"
            .parse::<Input>()
            .expect("parse public records input");
    let private_input =
        "(PrivateRecords ((Partial [(Technology (Software (Data SchemaEvolution)))]) None))"
            .parse::<Input>()
            .expect("parse private records input");

    assert_eq!(
        public_input,
        Input::public_records(RecordSelection {
            domain_match: DomainMatch::full(domain_fixtures::scopes(&["schema"])),
            selected_kind: SelectedKind::new(Some(Kind::Decision)),
        })
    );
    assert_eq!(
        private_input,
        Input::private_records(RecordSelection {
            domain_match: DomainMatch::partial(domain_fixtures::scopes(&["schema"])),
            selected_kind: SelectedKind::new(None),
        })
    );
    assert_eq!(
        public_input.to_string(),
        "(PublicRecords ((Full [(Technology (Software (Data SchemaEvolution)))]) (Some Decision)))"
    );
    assert_eq!(
        private_input.to_string(),
        "(PrivateRecords ((Partial [(Technology (Software (Data SchemaEvolution)))]) None))"
    );
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_public_intent_round_trips_recursive_domain_scopes() {
    use spirit::schema::signal::{
        DataLeaf, Domain, DomainScope, DomainScopes, Software, Technology,
    };

    let input = "(PublicIntent [(Technology (Software (Data SchemaEvolution)))])"
        .parse::<Input>()
        .expect("parse public intent input");
    let impossible = "(PublicIntent [(Technology Software)])".parse::<Input>();

    let schema_evolution_domain = Domain::Technology(Technology::Software(Software::Data(
        DataLeaf::SchemaEvolution,
    )));
    let schema_evolution_scope = DomainScope::from(schema_evolution_domain.clone());

    assert!(impossible.is_err());
    assert_eq!(
        input,
        Input::public_intent(DomainScopes::new(vec![schema_evolution_scope.clone()]))
    );
    assert!(schema_evolution_scope.contains_domain(&schema_evolution_domain));
    assert_eq!(
        input.to_string(),
        "(PublicIntent [(Technology (Software (Data SchemaEvolution)))])"
    );
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_record_input_renders_bracket_bearing_strings_losslessly() {
    let description = String::from("text contains [brackets] and the pipe close marker |]");
    let entry = Entry {
        domains: domain_fixtures::domains(&["schema replay"]),
        kind: Kind::Correction,
        description: Description::new(description.clone()),
        certainty: Magnitude::High.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: spirit::schema::signal::Referents::new(vec![
            spirit::schema::signal::Referent::new("spirit"),
        ]),
    };
    let input = Input::record(record_request(entry, &description));
    let rendered = input.to_string();

    assert_eq!(
        rendered,
        "(Record (([(Technology (Software (Data SchemaEvolution)))] Correction [|text contains [brackets] and the pipe close marker \\|]|] High Minimum Zero [spirit]) ([([|text contains [brackets] and the pipe close marker \\|]|] None)] [|text contains [brackets] and the pipe close marker \\|]|])))"
    );
    let reparsed = rendered
        .parse::<Input>()
        .expect("generated Input should parse its own bracket-safe NOTA");
    assert_eq!(reparsed, input);
}

#[test]
fn generated_rejection_output_is_a_signal_schema_variant() {
    let output = Output::rejected(SignalRejection::new(ValidationError::EmptyDomain));

    assert_eq!(output.route(), OutputRoute::Rejected);
    #[cfg(feature = "nota-text")]
    assert_eq!(output.to_string(), "(Rejected EmptyDomain)");

    let frame = output.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Output::decode_signal_frame(&frame).expect("decode frame");

    assert_eq!(route, OutputRoute::Rejected);
    assert_eq!(decoded, output);
}

#[test]
fn bare_schema_bindings_are_explicit_payload_wrappers() {
    let entry = Entry {
        domains: domain_fixtures::domains(&["schema"]),
        kind: Kind::Constraint,
        description: Description::new("alias bindings carry direct payloads"),
        certainty: Magnitude::Maximum.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: spirit::schema::signal::Referents::new(vec![
            spirit::schema::signal::Referent::new("spirit"),
        ]),
    };
    let record: RecordInput = record_request(entry, "alias bindings carry direct payloads").into();
    let input = Input::Record(record);
    match input {
        Input::Record(record) => {
            assert_eq!(
                record.payload().entry.description.payload(),
                "alias bindings carry direct payloads"
            );
        }
        other => panic!("expected Record wrapper payload, got {other:?}"),
    }

    let rejected: RejectedOutput = SignalRejection::new(ValidationError::EmptyDomain).into();
    let output = Output::Rejected(rejected);
    match output {
        Output::Rejected(rejection) => {
            assert_eq!(rejection.payload().payload(), &ValidationError::EmptyDomain);
        }
        other => panic!("expected Rejected wrapper payload, got {other:?}"),
    }
}

#[cfg(feature = "nota-text")]
#[test]
fn generated_validation_error_round_trips_through_nota() {
    let rejection = "(Rejected EmptyDescription)"
        .parse::<Output>()
        .expect("parse rejection output");

    assert_eq!(
        rejection,
        Output::rejected(SignalRejection::new(ValidationError::EmptyDescription))
    );
    assert_eq!(rejection.to_string(), "(Rejected EmptyDescription)");
}

#[test]
fn generated_signal_surface_rejects_unknown_header_before_body_decode() {
    let entry = Entry {
        domains: domain_fixtures::domains(&["schema"]),
        kind: Kind::Constraint,
        description: Description::new("schema rejects unknown routes"),
        certainty: Magnitude::Maximum.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: spirit::schema::signal::Referents::new(vec![
            spirit::schema::signal::Referent::new("spirit"),
        ]),
    };
    let mut frame = Input::record(record_request(entry, "schema rejects unknown routes"))
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
fn signal_admission_emits_mail_sent_event() {
    let entry = Entry {
        domains: domain_fixtures::domains(&["schema"]),
        kind: Kind::Constraint,
        description: Description::new("signal admission emits mail events"),
        certainty: Magnitude::Maximum.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: spirit::schema::signal::Referents::new(vec![
            spirit::schema::signal::Referent::new("spirit"),
        ]),
    };
    let input = Input::record(record_request(entry, "signal admission emits mail events"));
    let short_header = input.short_header();

    let accepted = SignalAdmission::default()
        .admit(input)
        .expect("valid input admits");
    let event = accepted.message_sent();

    assert_eq!(event.identifier.payload(), 1);
    assert_eq!(event.origin_route(), OriginRoute::new(1_000_001));
    assert_ne!(
        event.origin_route(),
        OriginRoute::new(event.identifier.payload())
    );
    assert_eq!(event.short_header, short_header);
}
