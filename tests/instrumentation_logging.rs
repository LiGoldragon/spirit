use spirit_next::{
    CommitSequence, DatabaseMarker, Description, Engine, Entry, Kind, Magnitude, Output,
    RecordIdentifier, SemaReadInput, SemaReadOutput, SemaWriteInput, SemaWriteOutput, StateDigest,
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
    }
}

#[test]
fn testing_trace_records_real_signal_nexus_and_sema_trait_calls() {
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
    }));
    assert!(matches!(observed.root(), Output::RecordsObserved(_)));

    let events = trace_log.events();
    assert_eq!(
        events.len(),
        10,
        "record and observe each cross five traced runtime boundaries"
    );

    match &events[0] {
        TraceEvent::SignalAdmission {
            origin_route,
            input,
        } => {
            assert_eq!(*origin_route, recorded.origin_route());
            assert!(matches!(input, spirit_next::Input::Record(_)));
        }
        other => panic!("expected signal admission event, got {other:?}"),
    }
    match &events[1] {
        TraceEvent::NexusExecute {
            origin_route,
            input,
        } => {
            assert_eq!(*origin_route, recorded.origin_route());
            assert!(matches!(input, spirit_next::NexusInput::Signal(_)));
        }
        other => panic!("expected nexus entry event, got {other:?}"),
    }
    match &events[2] {
        TraceEvent::NexusDecision {
            origin_route,
            output,
        } => {
            assert_eq!(*origin_route, recorded.origin_route());
            assert!(matches!(output, spirit_next::NexusOutput::SemaWrite(_)));
        }
        other => panic!("expected nexus decision event, got {other:?}"),
    }
    match &events[3] {
        TraceEvent::SemaApply {
            origin_route,
            input,
            output,
        } => {
            assert_eq!(*origin_route, recorded.origin_route());
            assert!(matches!(input, SemaWriteInput::Record(_)));
            match output {
                SemaWriteOutput::Recorded(receipt) => {
                    assert_eq!(receipt.record_identifier, RecordIdentifier(1));
                    assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(1));
                }
                other => panic!("expected recorded SEMA output, got {other:?}"),
            }
        }
        other => panic!("expected SEMA write event, got {other:?}"),
    }
    match &events[4] {
        TraceEvent::SignalReply {
            origin_route,
            output,
        } => {
            assert_eq!(*origin_route, recorded.origin_route());
            assert!(matches!(output, Output::RecordAccepted(_)));
        }
        other => panic!("expected signal reply event, got {other:?}"),
    }

    match &events[5] {
        TraceEvent::SignalAdmission {
            origin_route,
            input,
        } => {
            assert_eq!(*origin_route, observed.origin_route());
            assert!(matches!(input, spirit_next::Input::Observe(_)));
        }
        other => panic!("expected observe signal admission event, got {other:?}"),
    }
    match &events[6] {
        TraceEvent::NexusExecute { input, .. } => {
            assert!(matches!(input, spirit_next::NexusInput::Signal(_)));
        }
        other => panic!("expected observe nexus entry event, got {other:?}"),
    }
    match &events[7] {
        TraceEvent::NexusDecision { output, .. } => {
            assert!(matches!(output, spirit_next::NexusOutput::SemaRead(_)));
        }
        other => panic!("expected observe nexus decision event, got {other:?}"),
    }
    match &events[8] {
        TraceEvent::SemaObserve { input, output, .. } => {
            assert!(matches!(input, SemaReadInput::Observe(_)));
            assert!(matches!(output, SemaReadOutput::Observed(_)));
        }
        other => panic!("expected SEMA read event, got {other:?}"),
    }
    match &events[9] {
        TraceEvent::SignalReply { output, .. } => {
            assert!(matches!(output, Output::RecordsObserved(_)));
        }
        other => panic!("expected observe signal reply event, got {other:?}"),
    }

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
fn testing_trace_records_signal_rejection_without_nexus_or_sema_events() {
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
    assert_eq!(
        trace_log.events(),
        vec![
            TraceEvent::SignalRejection {
                origin_route: output.origin_route(),
                validation_error: ValidationError::EmptyTopic,
            },
            TraceEvent::SignalReply {
                origin_route: output.origin_route(),
                output: output.root().clone(),
            },
        ],
        "invalid Signal input is rejected and replied to without reaching Nexus or SEMA"
    );
}
