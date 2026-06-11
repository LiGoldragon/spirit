#![cfg(feature = "agent-guardian")]

use std::{
    io::Write,
    os::unix::net::{UnixListener, UnixStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use agent::{
    AgentEngine,
    provider::OpenAiCompatibleProvider,
    registry::{ProviderEntry, ProviderRegistry, SecretSource},
};
use signal_agent::Input as AgentInput;
use spirit::{
    AgentGuardian, AgentGuardianConfiguration, Engine, Store,
    schema::signal::{
        Description, Domains, Entry, GuardianRejectionReason, Input, Kind, Magnitude, Output,
        Privacy, Proposal, RecordRequest, Referents, StatementText,
    },
};
use tempfile::TempDir;
use triad_runtime::{FrameBody, LengthPrefixedCodec};

const DEEPSEEK_PROVIDER: &str = "deepseek";
const DEEPSEEK_ENDPOINT: &str = "https://api.deepseek.com/v1";
const DEEPSEEK_MODEL: &str = "deepseek-v4-flash";
const DEEPSEEK_GOPASS_PATH: &str = "platform.deepseek.com/api-key";

struct LiveAgentServer {
    _directory: TempDir,
    socket_path: std::path::PathBuf,
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

struct GuardianScenario {
    name: &'static str,
    proposal: Entry,
    justification: String,
    expected: ExpectedVerdict,
}

enum ExpectedVerdict {
    Accept,
    Reject(&'static [GuardianRejectionReason]),
}

impl LiveAgentServer {
    fn spawn(call_count: usize) -> Self {
        let directory = TempDir::new().expect("agent tempdir");
        let socket_path = directory.path().join("agent.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind live agent socket");
        listener
            .set_nonblocking(true)
            .expect("live agent listener nonblocking");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let mut registry = ProviderRegistry::new();
            registry.configure(ProviderEntry::new(
                DEEPSEEK_PROVIDER,
                DEEPSEEK_ENDPOINT,
                DEEPSEEK_MODEL,
                SecretSource::gopass(DEEPSEEK_GOPASS_PATH),
            ));
            let mut engine =
                AgentEngine::with_system_keys(registry, Box::new(OpenAiCompatibleProvider::new()));
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("agent runtime");
            let mut served = 0;
            while served < call_count && !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        served += 1;
                        Self::answer(stream, &mut engine, &runtime);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => panic!("accept agent call: {error}"),
                }
            }
        });
        Self {
            _directory: directory,
            socket_path,
            stop,
            thread,
        }
    }

    fn answer(mut stream: UnixStream, engine: &mut AgentEngine, runtime: &tokio::runtime::Runtime) {
        let codec = LengthPrefixedCodec::default();
        let request = codec
            .read_body(&mut stream)
            .expect("read agent request")
            .into_bytes();
        let (_route, input) =
            AgentInput::decode_signal_frame(&request).expect("decode agent input");
        let output = runtime.block_on(engine.handle(input));
        codec
            .write_body(
                &mut stream,
                &FrameBody::new(output.encode_signal_frame().expect("encode agent output")),
            )
            .expect("write agent output");
        stream.flush().expect("flush agent output");
    }

    fn guardian(&self) -> AgentGuardian {
        AgentGuardian::new(AgentGuardianConfiguration::new(
            self.socket_path.clone(),
            Some(DEEPSEEK_PROVIDER.to_owned()),
            Some(DEEPSEEK_MODEL.to_owned()),
            Duration::from_secs(120),
            None,
        ))
    }

    fn join(self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().expect("live agent server joins");
    }
}

impl GuardianScenario {
    fn accepts(name: &'static str, proposal: Entry) -> Self {
        let justification = proposal.description.payload().clone();
        Self {
            name,
            proposal,
            justification,
            expected: ExpectedVerdict::Accept,
        }
    }

    fn accepts_with_justification(
        name: &'static str,
        proposal: Entry,
        justification: &'static str,
    ) -> Self {
        Self {
            name,
            proposal,
            justification: justification.to_owned(),
            expected: ExpectedVerdict::Accept,
        }
    }

    fn rejects(
        name: &'static str,
        proposal: Entry,
        allowed_reasons: &'static [GuardianRejectionReason],
    ) -> Self {
        let justification = proposal.description.payload().clone();
        Self {
            name,
            proposal,
            justification,
            expected: ExpectedVerdict::Reject(allowed_reasons),
        }
    }

    fn rejects_with_justification(
        name: &'static str,
        proposal: Entry,
        justification: &'static str,
        allowed_reasons: &'static [GuardianRejectionReason],
    ) -> Self {
        Self {
            name,
            proposal,
            justification: justification.to_owned(),
            expected: ExpectedVerdict::Reject(allowed_reasons),
        }
    }
}

#[test]
#[ignore = "uses live DeepSeek through gopass; run explicitly before guardian deployment"]
fn live_deepseek_guardian_accepts_and_rejects_realistic_scenarios() {
    if !DeepSeekKey::available() {
        eprintln!("skipping: DeepSeek gopass key unavailable");
        return;
    }

    let directory = TempDir::new().expect("spirit tempdir");
    let store = Store::open(directory.path().join("guardian-live.sema")).expect("open store");
    let mut engine = Engine::new(store);
    for seed in seed_records() {
        assert!(matches!(
            engine.handle(Input::record(record_request(seed))).root(),
            Output::RecordAccepted(_)
        ));
    }

    let scenarios = scenarios();
    let live_agent = LiveAgentServer::spawn(scenarios.len() * 2);
    engine.set_guardian(live_agent.guardian());

    for scenario in scenarios {
        let output = engine.handle(Input::propose(proposal(
            scenario.proposal,
            scenario.justification.as_str(),
        )));
        match scenario.expected {
            ExpectedVerdict::Accept => {
                assert!(
                    matches!(output.root(), Output::Proposed(_)),
                    "{} should have been accepted, got {output:?}",
                    scenario.name
                );
            }
            ExpectedVerdict::Reject(allowed_reasons) => match output.root() {
                Output::GuardianRejected(rejection) => {
                    assert!(
                        allowed_reasons.contains(&rejection.payload().guardian_rejection_reason),
                        "{} rejected for {:?}, expected one of {:?}; rejection: {:?}",
                        scenario.name,
                        rejection.payload().guardian_rejection_reason,
                        allowed_reasons,
                        rejection
                    );
                }
                other => panic!("{} should have been rejected, got {other:?}", scenario.name),
            },
        }
    }

    live_agent.join();
}

struct DeepSeekKey;

impl DeepSeekKey {
    fn available() -> bool {
        std::process::Command::new("gopass")
            .arg("show")
            .arg("-o")
            .arg(DEEPSEEK_GOPASS_PATH)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}

fn seed_records() -> Vec<Entry> {
    vec![
        entry(
            &["software", "nota"],
            "NOTA strings are represented with bracket forms; quotation marks are not valid NOTA string syntax.",
        ),
        entry(
            &["software", "spirit"],
            "Spirit intent entries express one forward act, principle, constraint, or decision at a time.",
        ),
        entry(
            &["software", "agent"],
            "Agent provider secrets are resolved by the agent daemon from configured secret-source backends.",
        ),
        entry(
            &["software", "spirit"],
            "Referents must be registered before records may attach them to entries.",
        ),
    ]
}

fn scenarios() -> Vec<GuardianScenario> {
    vec![
        GuardianScenario::accepts(
            "clear guardian testing intent",
            entry(
                &["software", "spirit"],
                "Spirit guardian tests should use sandbox stores with realistic accept and reject proposals before deployment.",
            ),
        ),
        GuardianScenario::accepts(
            "clear agent provider test intent",
            entry(
                &["software", "agent"],
                "Agent live-provider tests should verify gopass-backed DeepSeek calls through the same signal protocol Spirit uses.",
            ),
        ),
        GuardianScenario::accepts_with_justification(
            "detailed justification is source evidence",
            entry(
                &["software", "spirit"],
                "Spirit guardian admission should judge the Entry as the candidate intent and use Justification only as source evidence.",
            ),
            "This regression test exists because the current Proposal shape carries both an Entry and a Justification. The admission question is only whether the Entry can enter the live intent store; this paragraph explains why the test matters and must not be counted as a second intent arrow.",
        ),
        GuardianScenario::rejects_with_justification(
            "duplicate provider secret policy",
            entry(
                &["software", "agent"],
                "Agent provider secrets are resolved by the agent daemon from configured secret-source backends.",
            ),
            "This repeats a seeded record exactly so the model can reject it as a duplicate instead of treating the justification text as the candidate.",
            &[GuardianRejectionReason::Duplicate],
        ),
        GuardianScenario::rejects(
            "nota quotation contradiction",
            entry(
                &["software", "nota"],
                "NOTA strings should use quotation marks as the canonical representation.",
            ),
            &[GuardianRejectionReason::Contradiction],
        ),
        GuardianScenario::rejects(
            "compound agent and deployment intent",
            entry(
                &["software", "agent"],
                "Agent should resolve DeepSeek keys through gopass and Spirit should deploy the guardian immediately.",
            ),
            &[GuardianRejectionReason::Compound],
        ),
        GuardianScenario::rejects(
            "non-intent uncertainty",
            entry(
                &["software", "spirit"],
                "I am unsure whether the guardian is ready.",
            ),
            &[GuardianRejectionReason::NonIntent],
        ),
    ]
}

fn entry(domains: &[&str], description: &str) -> Entry {
    Entry {
        domains: Domains::from_strings(domains.iter().map(|domain| (*domain).to_owned()).collect()),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: Magnitude::Maximum.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: Referents::new(Vec::new()),
    }
}

fn record_request(entry: Entry) -> RecordRequest {
    let statement = entry.description.payload().clone();
    RecordRequest {
        entry,
        justification: justification(&statement),
    }
}

fn proposal(entry: Entry, justification_text: &str) -> Proposal {
    Proposal {
        entry,
        justification: justification(justification_text),
    }
}

fn justification(statement: &str) -> spirit::schema::signal::Justification {
    spirit::schema::signal::Justification {
        statement_text: StatementText::new(statement),
        context: None,
    }
}
