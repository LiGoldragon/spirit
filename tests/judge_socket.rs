#![cfg(feature = "agent-guardian")]

mod support;

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use signal_frame::{ExchangeFrameBody, NonEmpty, Reply, ShortHeader, SubReply};
use signal_spirit_judge::{
    AdmissionJudgeOperation, AdmissionJudgeResponse, AdmissionJudgeVerdict, JudgeDiagnostic,
    PrivateDiagnosticPolicy, RedactedText, ReferentRegistrationJudgeResponse,
    ReferentRegistrationJudgeVerdict, SpiritJudgeFrame, SpiritJudgeReply, SpiritJudgeRequest,
};
use spirit::{
    AgentGuardian, AgentGuardianConfiguration, Engine, Store,
    schema::signal::{
        Clarification, ClarificationRecordIdentifier, ClarificationResolution, Description,
        Domains, Entry, GuardianRejectionReason, Importance, Input, Justification, Kind, Magnitude,
        Output, Privacy, Proposal, QuoteText, Reasoning, RecordChange, RecordIdentifier,
        RecordRequest, Referents, Replacements, RetiredIdentifier, RetiredIdentifiers, Retirement,
        Supersession, TargetClarification, TargetClarifications, Testimony, VerbatimQuote,
    },
};
use tempfile::TempDir;

use support::domain_fixtures;

struct FakeSpiritJudge {
    directory: TempDir,
    captured_requests: Arc<Mutex<Vec<SpiritJudgeRequest>>>,
    thread: thread::JoinHandle<()>,
}

impl FakeSpiritJudge {
    fn accept_once() -> Self {
        Self::accepting(2)
    }

    fn accepting(request_count: usize) -> Self {
        let directory = TempDir::new().expect("tempdir");
        let socket_path = directory.path().join("spirit-judge.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake spirit judge");
        let captured_requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&captured_requests);
        let thread = thread::spawn(move || {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().expect("accept judge request");
                let frame = FrameIo::new(&mut stream).read_frame();
                let ExchangeFrameBody::Request { exchange, request } = frame.into_body() else {
                    panic!("expected judge request frame");
                };
                let request_payload = request.payloads().head().clone();
                thread_requests
                    .lock()
                    .expect("capture request")
                    .push(request_payload.clone());
                let reply = match request_payload {
                    SpiritJudgeRequest::JudgeAdmission(_) => {
                        SpiritJudgeReply::AdmissionJudged(AdmissionJudgeResponse::new(
                            AdmissionJudgeVerdict::Accept,
                            JudgeDiagnostic::redacted(
                                RedactedText::new("accepted").expect("static diagnostic"),
                            ),
                        ))
                    }
                    SpiritJudgeRequest::JudgeReferentRegistration(_) => {
                        SpiritJudgeReply::ReferentRegistrationJudged(
                            ReferentRegistrationJudgeResponse::new(
                                ReferentRegistrationJudgeVerdict::Accept,
                                JudgeDiagnostic::redacted(
                                    RedactedText::new("accepted").expect("static diagnostic"),
                                ),
                            ),
                        )
                    }
                };
                let frame = SpiritJudgeFrame::with_short_header(
                    ShortHeader::empty(),
                    ExchangeFrameBody::Reply {
                        exchange,
                        reply: Reply::committed(
                            NonEmpty::try_from_vec(vec![SubReply::Ok(reply)])
                                .expect("reply list is non-empty"),
                        ),
                    },
                );
                FrameIo::new(&mut stream).write_frame(&frame);
            }
        });
        Self {
            directory,
            captured_requests,
            thread,
        }
    }

    fn guardian(&self) -> AgentGuardian {
        AgentGuardian::new(AgentGuardianConfiguration::new(
            self.directory.path().join("spirit-judge.sock"),
            None,
            None,
            Duration::from_secs(5),
            None,
        ))
    }

    fn join(self) -> Vec<SpiritJudgeRequest> {
        self.thread.join().expect("fake judge joins");
        self.captured_requests
            .lock()
            .expect("read requests")
            .clone()
    }
}

struct FrameIo<'stream> {
    stream: &'stream mut UnixStream,
}

impl<'stream> FrameIo<'stream> {
    fn new(stream: &'stream mut UnixStream) -> Self {
        Self { stream }
    }

    fn read_frame(&mut self) -> SpiritJudgeFrame {
        let mut prefix = [0_u8; 4];
        self.stream.read_exact(&mut prefix).expect("read prefix");
        let length = u32::from_be_bytes(prefix) as usize;
        let mut bytes = Vec::with_capacity(4 + length);
        bytes.extend_from_slice(&prefix);
        bytes.resize(4 + length, 0);
        self.stream.read_exact(&mut bytes[4..]).expect("read body");
        SpiritJudgeFrame::decode_length_prefixed(bytes.as_slice()).expect("decode frame")
    }

    fn write_frame(&mut self, frame: &SpiritJudgeFrame) {
        let bytes = frame.encode_length_prefixed().expect("encode frame");
        self.stream
            .write_all(bytes.as_slice())
            .expect("write frame");
        self.stream.flush().expect("flush frame");
    }
}

#[test]
fn daemon_admission_sends_typed_spirit_judge_request_with_scope() {
    let judge = FakeSpiritJudge::accept_once();
    let database = TempDir::new().expect("database tempdir");
    let mut engine = Engine::new(Store::open(database.path().join("intent.sema")).expect("store"));
    engine.set_guardian(judge.guardian());

    let output = engine.handle(Input::record(record_request(entry_with_privacy(
        "typed judge request crosses the daemon boundary",
        Magnitude::Minimum,
    ))));

    assert!(
        matches!(output.root(), Output::RecordAccepted(_)),
        "expected record accepted, got {:?}",
        output.root()
    );
    let requests = judge.join();
    assert!(
        matches!(
            requests.first(),
            Some(SpiritJudgeRequest::JudgeReferentRegistration(_))
        ),
        "the implied referent crosses the same typed judge socket first: {requests:?}"
    );
    let Some(SpiritJudgeRequest::JudgeAdmission(packet)) = requests.get(1) else {
        panic!("expected the second judge request to be admission: {requests:?}");
    };
    assert!(matches!(
        &packet.scope,
        signal_spirit_judge::JudgmentScope::Private(private)
            if matches!(private.diagnostic_policy, PrivateDiagnosticPolicy::HashesAndRedaction)
    ));
    assert!(matches!(
        packet.operation,
        signal_spirit_judge::AdmissionJudgeOperation::Record(_)
    ));
}

#[test]
fn required_guardian_rejects_proposal_when_judge_is_unconfigured() {
    let database = TempDir::new().expect("database tempdir");
    let mut engine = Engine::new(Store::open(database.path().join("intent.sema")).expect("store"));
    let setup_identifier = accept_record(
        &mut engine,
        entry_with_privacy("registered referent setup", Magnitude::Zero),
    );
    engine.require_guardian();

    let output = engine.handle(input_propose(entry_with_privacy(
        "unguarded proposal should fail closed",
        Magnitude::Zero,
    )));

    match output.root() {
        Output::GuardianRejected(rejection) => {
            assert_eq!(
                rejection.payload().guardian_rejection_reason,
                GuardianRejectionReason::HarnessUnavailable
            );
            assert_eq!(
                rejection.payload().explanation.payload(),
                "guardian is required but no guardian agent is configured"
            );
        }
        other => panic!("expected GuardianRejected for missing proposal judge, got {other:?}"),
    }
    assert_eq!(engine.record_count(), 1);
    assert!(!setup_identifier.payload().is_empty());
    assert_eq!(engine.guardian_decision_count(), 1);
}

#[test]
fn daemon_admission_scope_uses_private_existing_record_context() {
    let judge = FakeSpiritJudge::accepting(5);
    let database = TempDir::new().expect("database tempdir");
    let mut engine = Engine::new(Store::open(database.path().join("intent.sema")).expect("store"));

    let clarify_identifier = accept_record(
        &mut engine,
        entry_with_privacy("private clarify context", Magnitude::Minimum),
    );
    let change_identifier = accept_record(
        &mut engine,
        entry_with_privacy("private change context", Magnitude::Minimum),
    );
    let supersede_identifier = accept_record(
        &mut engine,
        entry_with_privacy("private supersede context", Magnitude::Minimum),
    );
    let retire_identifier = accept_record(
        &mut engine,
        entry_with_privacy("private retire context", Magnitude::Minimum),
    );
    let resolution_identifier = accept_record(
        &mut engine,
        entry_with_privacy("private resolution context", Magnitude::Minimum),
    );
    let resolution_target_identifier = accept_record(
        &mut engine,
        entry_with_privacy("private resolution target", Magnitude::Minimum),
    );
    engine.set_guardian(judge.guardian());

    let clarified = engine.handle(input_clarify(
        clarify_identifier,
        "redacted clarify replacement",
    ));
    assert!(matches!(clarified.root(), Output::Clarified(_)));
    let changed = engine.handle(input_change_record(
        change_identifier,
        entry_with_privacy("public change replacement", Magnitude::Zero),
    ));
    assert!(matches!(changed.root(), Output::RecordChanged(_)));
    let superseded = engine.handle(input_supersede(
        supersede_identifier,
        entry_with_privacy("public supersede replacement", Magnitude::Zero),
    ));
    assert!(matches!(superseded.root(), Output::Superseded(_)));
    let retired = engine.handle(input_retire(retire_identifier));
    assert!(matches!(retired.root(), Output::Retired(_)));
    let resolved = engine.handle(input_resolve_clarification(
        resolution_identifier,
        resolution_target_identifier,
        "redacted resolution replacement",
    ));
    assert!(matches!(resolved.root(), Output::ClarificationResolved(_)));

    let requests = judge.join();
    assert_eq!(
        requests.len(),
        5,
        "expected one admission request per operation"
    );
    assert!(matches!(
        private_admission_operation(&requests[0]),
        AdmissionJudgeOperation::Clarify(_)
    ));
    assert!(matches!(
        private_admission_operation(&requests[1]),
        AdmissionJudgeOperation::ChangeRecord(_)
    ));
    assert!(matches!(
        private_admission_operation(&requests[2]),
        AdmissionJudgeOperation::Supersede(_)
    ));
    assert!(matches!(
        private_admission_operation(&requests[3]),
        AdmissionJudgeOperation::Retire(_)
    ));
    assert!(matches!(
        private_admission_operation(&requests[4]),
        AdmissionJudgeOperation::ResolveClarification(_)
    ));
}

fn private_admission_operation(request: &SpiritJudgeRequest) -> &AdmissionJudgeOperation {
    let SpiritJudgeRequest::JudgeAdmission(packet) = request else {
        panic!("expected admission judge request, got {request:?}");
    };
    assert!(matches!(
        &packet.scope,
        signal_spirit_judge::JudgmentScope::Private(private)
            if matches!(private.diagnostic_policy, PrivateDiagnosticPolicy::HashesAndRedaction)
    ));
    &packet.operation
}

fn accept_record(engine: &mut Engine, entry: Entry) -> RecordIdentifier {
    match engine.handle(input_record(entry)).into_root() {
        Output::RecordAccepted(identifier) => identifier.into_payload(),
        other => panic!("expected setup record accepted, got {other:?}"),
    }
}

fn entry_with_privacy(description: &str, privacy: Magnitude) -> Entry {
    Entry {
        domains: Domains::new(domain_fixtures::domains(&["judge-socket"]).into_payload()),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: Magnitude::Maximum.into(),
        importance: Importance::new(Magnitude::Minimum),
        privacy: Privacy::new(privacy),
        referents: Referents::new(vec![spirit::schema::signal::Referent::new("spirit")]),
    }
}

fn input_record(entry: Entry) -> Input {
    Input::record(record_request(entry))
}

fn input_propose(entry: Entry) -> Input {
    Input::propose(Proposal {
        entry,
        justification: justification("proposed forward arrow"),
    })
}

fn input_clarify(record_identifier: RecordIdentifier, description: &str) -> Input {
    Input::clarify(Clarification {
        record_identifier,
        description: Description::new(description),
        justification: justification(description),
    })
}

fn input_change_record(record_identifier: RecordIdentifier, entry: Entry) -> Input {
    Input::change_record(RecordChange {
        record_identifier,
        entry,
        justification: justification("change record"),
    })
}

fn input_supersede(record_identifier: RecordIdentifier, replacement: Entry) -> Input {
    Input::supersede(Supersession {
        retired_identifiers: RetiredIdentifiers::new(vec![RetiredIdentifier::new(
            record_identifier,
        )]),
        replacements: Replacements::new(vec![replacement]),
        justification: justification("replacement forward arrow"),
    })
}

fn input_retire(record_identifier: RecordIdentifier) -> Input {
    Input::retire(Retirement {
        record_identifier,
        justification: justification("retire this record"),
    })
}

fn input_resolve_clarification(
    clarification_identifier: RecordIdentifier,
    target_identifier: RecordIdentifier,
    description: &str,
) -> Input {
    Input::resolve_clarification(ClarificationResolution {
        clarification_record_identifier: ClarificationRecordIdentifier::new(
            clarification_identifier,
        ),
        target_clarifications: TargetClarifications::new(vec![TargetClarification {
            record_identifier: target_identifier,
            description: Description::new(description),
        }]),
        justification: justification("resolve clarification"),
    })
}

fn record_request(entry: Entry) -> RecordRequest {
    let statement = entry.description.payload().clone();
    RecordRequest {
        entry,
        justification: justification(&statement),
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
