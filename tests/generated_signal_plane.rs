mod support;

#[cfg(feature = "nota-text")]
use nota::{NotaEncode, NotaSource};
use spirit::schema::signal::{
    Description, Entry, Input, InputRoute, Justification, Kind, Magnitude, Output, OutputRoute,
    QuoteText, Reasoning, RecordIdentifier, RecordRequest, SearchText, Testimony, VerbatimQuote,
};
use support::domain_fixtures;

fn entry() -> Entry {
    Entry {
        domains: domain_fixtures::domains(&["schema"]),
        kind: Kind::Constraint,
        description: Description::new("schema creates the v14 signal plane"),
        importance: Magnitude::Medium.into(),
    }
}

fn record_request() -> RecordRequest {
    RecordRequest {
        entry: entry(),
        justification: Justification {
            testimony: Testimony::new(vec![VerbatimQuote::new(
                QuoteText::new("schema creates the v14 signal plane"),
                None,
            )]),
            reasoning: Reasoning::new("schema creates the v14 signal plane"),
        },
    }
}

#[test]
fn generated_four_field_record_round_trips_its_revision_2_frame() {
    let input = Input::record(record_request());
    assert_eq!(input.route(), InputRoute::Record);
    let frame = input.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode frame");
    assert_eq!(route, InputRoute::Record);
    assert_eq!(decoded, input);
}

#[test]
fn generated_uniform_read_roots_round_trip_revision_2_frames() {
    let inputs = [
        Input::text_search(SearchText::new("schema")),
        Input::intent(domain_fixtures::scopes(&["schema"])),
    ];
    for input in inputs {
        let expected_route = input.route();
        let frame = input.encode_signal_frame().expect("encode read frame");
        let (route, decoded) = Input::decode_signal_frame(&frame).expect("decode read frame");
        assert_eq!(route, expected_route);
        assert_eq!(decoded, input);
    }
}

#[test]
fn generated_output_round_trips_its_revision_2_frame() {
    let output = Output::record_accepted(RecordIdentifier::new("003g"));
    assert_eq!(output.route(), OutputRoute::RecordAccepted);
    let frame = output.encode_signal_frame().expect("encode frame");
    let (route, decoded) = Output::decode_signal_frame(&frame).expect("decode frame");
    assert_eq!(route, OutputRoute::RecordAccepted);
    assert_eq!(decoded, output);
}

#[cfg(feature = "nota-text")]
#[test]
fn canonical_four_field_entry_round_trips_nota() {
    let rendered = entry().to_nota();
    let decoded: Entry = NotaSource::new(&rendered)
        .parse()
        .expect("decode four-field entry");
    assert_eq!(decoded, entry());
    assert_eq!(
        rendered,
        "([(Technology (Software (Data SchemaEvolution)))] Constraint [schema creates the v14 signal plane] Medium)"
    );
}

#[cfg(feature = "nota-text")]
#[test]
fn revision_1_roots_and_seven_field_entry_are_rejected() {
    for source in [
        "(PublicIntent [All])",
        "(PublicTextSearch [schema])",
        "(PublicRecords (Any None))",
        "(PrivateRecords (Any None))",
        "(ChangeCertainty (003g Zero))",
        "(RegisterReferent (spirit [] ([] [reason])))",
        "(Record (([All] Decision [description] High Medium Zero [spirit]) ([] [reason])))",
    ] {
        assert!(
            NotaSource::new(source).parse::<Input>().is_err(),
            "revision-1 syntax must fail: {source}"
        );
    }
}

#[test]
fn generated_artifacts_contain_no_retired_spirit_surface() {
    for retired in [
        "Certainty",
        "PrivacySelection",
        "ReferentSelection",
        "RegisterReferent",
        "ChangeCertainty",
        "PublicTextSearch",
        "PublicIntent",
        "PublicRecords",
        "PrivateRecords",
    ] {
        assert!(!signal_spirit::SIGNAL_SCHEMA_SOURCE.contains(retired));
        assert!(!signal_spirit::SIGNAL_RUST_SOURCE.contains(retired));
    }
}
