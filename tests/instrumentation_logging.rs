use spirit::{
    Engine, ObjectName, Store, TraceEvent, TraceLog,
    schema::{
        nexus::NexusObjectName,
        sema::SemaObjectName,
        signal::{
            DatabaseMarker, Entry, Input, Kind, Magnitude, Output, PrivacySelection, Query,
            SignalObjectName, SignalRejection, TopicMatch, ValidationError,
        },
    },
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
        Engine::new_with_trace(Store::open(&self.path).expect("open sema store"), trace_log)
    }
}

fn entry(description: &str) -> Entry {
    Entry {
        topics: vec![String::from("trace")],
        kind: Kind::Decision,
        description: String::from(description),
        magnitude: Magnitude::Maximum,
        privacy: Magnitude::Zero,
    }
}

#[test]
fn testing_trace_records_real_signal_nexus_and_sema_activations() {
    let sema = SemaFile::new();
    let trace_log = TraceLog::recording();
    let engine = sema.engine_with_trace(trace_log.clone());

    let recorded = engine.handle(Input::Record(entry("trace witness")));
    let record_marker = match recorded.root() {
        Output::RecordAccepted(receipt) => receipt.database_marker.clone(),
        other => panic!("expected RecordAccepted, got {other:?}"),
    };

    let observed = engine.handle(Input::Observe(Query {
        topic_match: TopicMatch::full(vec![String::from("trace")]),
        kind: Some(Kind::Decision),
        privacy_selection: PrivacySelection::default_observation_privacy(),
    }));
    // Designer 480: Observe now flows through Stash (operator 287 §
    // "Acceptance Tests"). The slim wire reply carries a handle, not the
    // full record set. The trace below witnesses the recursive Nexus loop:
    // NexusEntered fires ONCE per route — the loop runs SEMA + effect under
    // one Nexus activation.
    assert!(matches!(observed.root(), Output::RecordsStashed(_)));

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
    #[cfg(feature = "nota-text")]
    {
        let rendered = events[3].to_string();
        assert_eq!(rendered, "(Sema WriteApplied)");
        let parsed = rendered
            .parse::<TraceEvent>()
            .expect("trace event parses from generated NOTA");
        assert_eq!(parsed, events[3]);
    }
    assert_ne!(
        record_marker.state_digest, 0,
        "trace is attached to a real SEMA write, not a string-presence check"
    );
}

#[test]
fn testing_trace_records_lifecycle_hooks_from_generated_engine_traits() {
    let sema = SemaFile::new();
    let trace_log = TraceLog::recording();
    let engine = sema.engine_with_trace(trace_log.clone());

    engine.start().expect("start lifecycle hooks run");
    engine.stop().expect("stop lifecycle hooks run");

    assert_activation_names(
        &trace_log.events(),
        &[
            "SemaStarted",
            "NexusStarted",
            "SignalStarted",
            "SignalStopped",
            "NexusStopped",
            "SemaStopped",
        ],
    );
    assert_activation_objects(
        &trace_log.events(),
        &[
            ObjectName::Sema(SemaObjectName::Started),
            ObjectName::Nexus(NexusObjectName::Started),
            ObjectName::Signal(SignalObjectName::Started),
            ObjectName::Signal(SignalObjectName::Stopped),
            ObjectName::Nexus(NexusObjectName::Stopped),
            ObjectName::Sema(SemaObjectName::Stopped),
        ],
    );
}

#[test]
fn testing_trace_builds_record_activations_by_default() {
    let sema = SemaFile::new();
    let engine = Engine::new(Store::open(&sema.path).expect("open sema store"));

    let output = engine.handle(Input::Record(entry("default trace witness")));
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
    invalid_entry.topics = vec![];

    let output = engine.handle(Input::Record(invalid_entry));

    assert_eq!(engine.record_count(), 0);
    assert_eq!(
        output.root(),
        &Output::Rejected(SignalRejection {
            validation_error: ValidationError::EmptyTopic,
            database_marker: DatabaseMarker {
                commit_sequence: 0,
                state_digest: 0,
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
