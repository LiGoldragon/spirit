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
    AdmissionJudgeResponse, AdmissionJudgeVerdict, JudgeDiagnostic, PrivateDiagnosticPolicy,
    RedactedText, ReferentRegistrationJudgeResponse, ReferentRegistrationJudgeVerdict,
    SpiritJudgeFrame, SpiritJudgeReply, SpiritJudgeRequest,
};
use spirit::{
    AgentGuardian, AgentGuardianConfiguration, Engine, Store,
    schema::signal::{
        Description, Domains, Entry, Importance, Input, Justification, Kind, Magnitude, Output,
        Privacy, QuoteText, Reasoning, RecordRequest, Referents, Testimony, VerbatimQuote,
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
        let directory = TempDir::new().expect("tempdir");
        let socket_path = directory.path().join("spirit-judge.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake spirit judge");
        let captured_requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&captured_requests);
        let thread = thread::spawn(move || {
            for _ in 0..2 {
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

fn record_request(entry: Entry) -> RecordRequest {
    let statement = entry.description.payload().clone();
    RecordRequest {
        entry,
        justification: Justification {
            testimony: Testimony::new(vec![VerbatimQuote::new(
                QuoteText::new(statement.clone()),
                None,
            )]),
            reasoning: Reasoning::new(statement),
        },
    }
}
