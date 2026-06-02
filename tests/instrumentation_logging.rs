use spirit_next::{
    CommitSequence, DatabaseMarker, Description, Engine, Entry, Kind, Magnitude, NexusObjectName,
    ObjectName, Output, Privacy, PrivacySelection, SemaObjectName, SignalObjectName, StateDigest,
    Topic, TopicMatch, Topics, TraceEvent, TraceLog, ValidationError,
};
use tempfile::TempDir;

struct SemaFile {
    #[allow(dead_code)]
    directory: TempDir,
    path: std::path::PathBuf,
}

impl SemaFile {
    fn new() -> Self {
        let directory = TempDir::new().expect("tempdir");
        let path = directory.path().join("instrumentation.sema");
        Self { directory, path }
    }

    fn engine_with_trace(&self, trace_log: TraceLog) -> Engine {
        Engine::new_with_trace(
            spirit_next::Store::open(&self.path).expect("open sema store"),
            trace_log,
        )
    }
}

fn entry(description: &str) -> Entry {
    Entry {
        topics: Topics(vec![Topic(String::from("trace"))]),
        kind: Kind::Decision,
        description: Description(String::from(description)),
        magnitude: Magnitude::Maximum,
        privacy: Privacy(Magnitude::Zero),
    }
}

#[test]
fn testing_trace_records_real_signal_nexus_and_sema_activations() {
    let sema = SemaFile::new();
    let trace_log = TraceLog::recording();
    let engine = sema.engine_with_trace(trace_log.clone());

    let recorded = engine.handle(spirit_next::Input::Record(entry("trace witness")));
    let record_marker = match recorded.root() {
        Output::RecordAccepted(receipt) => receipt.database_marker.clone(),
        other => panic!("expected RecordAccepted, got {other:?}"),
    };

    let observed = engine.handle(spirit_next::Input::Observe(spirit_next::Query {
        topic_match: TopicMatch::Full(Topics(vec![Topic(String::from("trace"))])),
        kind: Some(Kind::Decision),
        privacy_selection: PrivacySelection::default_observation_privacy(),
    }));
    assert!(matches!(observed.root(), Output::RecordsObserved(_)));

    assert_activation_names(
        &trace_log.events(),
        &[
            "SignalAdmitted",
            "SignalTriaged",
            "NexusEntered",
            "SemaWriteApplied",
            "NexusDecided",
            "SignalReplied",
            "SignalAdmitted",
            "SignalTriaged",
            "NexusEntered",
            "SemaReadObserved",
            "NexusDecided",
            "SignalReplied",
        ],
    );

    let events = trace_log.events();
    assert_activation_objects(
        &events,
        &[
            ObjectName::Signal(SignalObjectName::Admitted),
            ObjectName::Signal(SignalObjectName::Triaged),
            ObjectName::Nexus(NexusObjectName::Entered),
            ObjectName::Sema(SemaObjectName::WriteApplied),
            ObjectName::Nexus(NexusObjectName::Decided),
            ObjectName::Signal(SignalObjectName::Replied),
            ObjectName::Signal(SignalObjectName::Admitted),
            ObjectName::Signal(SignalObjectName::Triaged),
            ObjectName::Nexus(NexusObjectName::Entered),
            ObjectName::Sema(SemaObjectName::ReadObserved),
            ObjectName::Nexus(NexusObjectName::Decided),
            ObjectName::Signal(SignalObjectName::Replied),
        ],
    );
    let archive =
        rkyv::to_bytes::<rkyv::rancor::Error>(&events[3]).expect("trace event archives as rkyv");
    let decoded = rkyv::from_bytes::<TraceEvent, rkyv::rancor::Error>(&archive)
        .expect("trace event decodes from rkyv");
    assert_eq!(decoded, events[3]);
    assert_ne!(
        record_marker.state_digest,
        StateDigest(0),
        "trace is attached to a real SEMA write, not a string-presence check"
    );
}

#[test]
fn testing_trace_builds_record_activations_by_default() {
    let sema = SemaFile::new();
    let engine = Engine::new(spirit_next::Store::open(&sema.path).expect("open sema store"));

    let output = engine.handle(spirit_next::Input::Record(entry("default trace witness")));
    assert!(matches!(output.root(), Output::RecordAccepted(_)));

    assert_activation_names(
        &engine.trace_events(),
        &[
            "SignalAdmitted",
            "SignalTriaged",
            "NexusEntered",
            "SemaWriteApplied",
            "NexusDecided",
            "SignalReplied",
        ],
    );
}

#[test]
fn testing_trace_records_signal_rejection_without_nexus_or_sema_activations() {
    let sema = SemaFile::new();
    let trace_log = TraceLog::recording();
    let engine = sema.engine_with_trace(trace_log.clone());

    let mut invalid_entry = entry("invalid trace witness");
    invalid_entry.topics = Topics(vec![]);

    let output = engine.handle(spirit_next::Input::Record(invalid_entry));

    assert_eq!(engine.record_count(), 0);
    assert_eq!(
        output.root(),
        &Output::Rejected(spirit_next::SignalRejection {
            validation_error: ValidationError::EmptyTopic,
            database_marker: DatabaseMarker {
                commit_sequence: CommitSequence(0),
                state_digest: StateDigest(0),
            },
        })
    );
    assert_activation_names(&trace_log.events(), &["SignalRejected", "SignalReplied"]);
}

fn assert_activation_names(events: &[TraceEvent], expected: &[&str]) {
    let actual = events.iter().map(TraceEvent::name).collect::<Vec<_>>();
    assert_eq!(actual, expected, "trace events: {events:#?}");
}

fn assert_activation_objects(events: &[TraceEvent], expected: &[ObjectName]) {
    let actual = events
        .iter()
        .map(TraceEvent::object_name)
        .collect::<Vec<ObjectName>>();
    let expected = expected.to_vec();
    assert_eq!(actual, expected, "trace events: {events:#?}");
}
