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
        Antecedent, Description, Domains, Entry, GuardianRejectionReason, Importance, Input, Kind,
        Magnitude, Output, Privacy, Proposal, QuoteText, Reasoning, RecordRequest, Referents,
        Testimony, VerbatimQuote,
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
    quote: &'static str,
    reasoning: &'static str,
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
    fn accepts(
        name: &'static str,
        proposal: Entry,
        quote: &'static str,
        reasoning: &'static str,
    ) -> Self {
        Self {
            name,
            proposal,
            quote,
            reasoning,
            expected: ExpectedVerdict::Accept,
        }
    }

    fn accepts_with_justification(
        name: &'static str,
        proposal: Entry,
        quote: &'static str,
        reasoning: &'static str,
    ) -> Self {
        Self {
            name,
            proposal,
            quote,
            reasoning,
            expected: ExpectedVerdict::Accept,
        }
    }

    fn rejects_with_justification(
        name: &'static str,
        proposal: Entry,
        quote: &'static str,
        reasoning: &'static str,
        allowed_reasons: &'static [GuardianRejectionReason],
    ) -> Self {
        Self {
            name,
            proposal,
            quote,
            reasoning,
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
            scenario.quote,
            scenario.reasoning,
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
            medium_entry(
                &["software", "spirit"],
                "Spirit guardian tests should use sandbox stores with realistic accept and reject proposals before deployment.",
            ),
            "we should use sandbox stores with realistic accept and reject proposals before deployment",
            "The quote is a direct testing preference; Medium matches should.",
        ),
        GuardianScenario::accepts(
            "clear agent provider test intent",
            medium_entry(
                &["software", "agent"],
                "Agent live-provider tests should verify gopass-backed DeepSeek calls through the same signal protocol Spirit uses.",
            ),
            "the live provider tests should go through gopass-backed DeepSeek using the same signal protocol Spirit uses",
            "The quote directly supports the live-provider test shape; Medium matches should.",
        ),
        GuardianScenario::accepts_with_justification(
            "detailed justification is source evidence",
            medium_entry(
                &["software", "spirit"],
                "Spirit guardian admission should judge the Entry as the candidate intent and use Justification only as source evidence.",
            ),
            "The guardian should judge the Entry itself; the justification is evidence for it, not a second intent.",
            "The quote distinguishes candidate Entry from evidentiary justification; Medium matches should.",
        ),
        GuardianScenario::rejects_with_justification(
            "duplicate provider secret policy",
            entry(
                &["software", "agent"],
                "Agent provider secrets are resolved by the agent daemon from configured secret-source backends.",
            ),
            "agent secrets come from the configured backends, that is settled",
            "Restates a settled seeded provider-secret policy.",
            &[GuardianRejectionReason::Duplicate],
        ),
        GuardianScenario::rejects_with_justification(
            "nota quotation contradiction",
            entry_with_certainty(
                &["software", "nota"],
                "NOTA strings should use quotation marks as the canonical representation.",
                Magnitude::High,
            ),
            "let us make quotation marks the canonical NOTA string form",
            "Contradicts the seeded NOTA bracket-form rule.",
            &[GuardianRejectionReason::Contradiction],
        ),
        GuardianScenario::rejects_with_justification(
            "compound agent and deployment intent",
            medium_entry(
                &["software", "agent"],
                "Agent should resolve DeepSeek keys through gopass and Spirit should deploy the guardian immediately.",
            ),
            "resolve the keys through gopass and also deploy the guardian right away",
            "Contains two separable arrows: key resolution and deployment.",
            &[GuardianRejectionReason::Compound],
        ),
        GuardianScenario::rejects_with_justification(
            "non-intent uncertainty",
            entry(
                &["software", "spirit"],
                "I am unsure whether the guardian is ready.",
            ),
            "I am not sure whether the guardian is ready, let me look again",
            "Momentary uncertainty is task state, not durable intent.",
            &[GuardianRejectionReason::NonIntent],
        ),
        GuardianScenario::rejects_with_justification(
            "negative spelling guideline",
            medium_entry(
                &["software", "spirit"],
                "Canonical prose names are criome for the authentication component and criomos for the operating system name; creome and creomos are misspellings.",
            ),
            "its criome and criomos, not creome and creomos",
            "Centers rejected spellings instead of an affirmative canonical naming rule.",
            &[GuardianRejectionReason::NegativeGuideline],
        ),
    ]
}

fn entry(domains: &[&str], description: &str) -> Entry {
    entry_with_certainty(domains, description, Magnitude::Maximum)
}

fn medium_entry(domains: &[&str], description: &str) -> Entry {
    entry_with_certainty(domains, description, Magnitude::Medium)
}

fn entry_with_certainty(domains: &[&str], description: &str, certainty: Magnitude) -> Entry {
    Entry {
        domains: Domains::from_strings(domains.iter().map(|domain| (*domain).to_owned()).collect()),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: certainty.into(),
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

fn proposal(entry: Entry, quote: &str, reasoning: &str) -> Proposal {
    Proposal {
        entry,
        justification: eval_justification(&[(quote, None)], reasoning),
    }
}

fn justification(statement: &str) -> spirit::schema::signal::Justification {
    spirit::schema::signal::Justification {
        testimony: Testimony::new(vec![VerbatimQuote::new(
            QuoteText::new(statement.to_owned()),
            None,
        )]),
        reasoning: Reasoning::new(statement.to_owned()),
    }
}

// ---- Flash-vs-Pro guardian evaluation -------------------------------------
//
// A measurement harness (not a pass/fail assertion) that runs discriminating
// scenarios through the real guardian against both DeepSeek models and prints a
// scorecard of exact-verdict and exact-reason agreement with the gold label. It
// is the empirical basis for picking the production model and for ablating the
// few-shot. Run explicitly:
//   cargo test --features agent-guardian eval_flash_vs_pro -- --ignored --nocapture

impl LiveAgentServer {
    fn guardian_with_model(&self, model: &str) -> AgentGuardian {
        AgentGuardian::new(AgentGuardianConfiguration::new(
            self.socket_path.clone(),
            Some(DEEPSEEK_PROVIDER.to_owned()),
            Some(model.to_owned()),
            Duration::from_secs(180),
            None,
        ))
    }
}

struct EvalCase {
    name: &'static str,
    entry: Entry,
    quotes: &'static [(&'static str, Option<&'static str>)],
    reasoning: &'static str,
    expected: EvalExpect,
}

enum EvalExpect {
    Accept,
    Reject(GuardianRejectionReason),
}

const EVAL_MODELS: [&str; 2] = ["deepseek-v4-flash", "deepseek-v4-pro"];

impl EvalCase {
    fn score(&self, output: &Output) -> (bool, bool, String) {
        match output {
            Output::Proposed(_) => {
                let accepted = matches!(self.expected, EvalExpect::Accept);
                (accepted, accepted, "Accept".to_owned())
            }
            Output::GuardianRejected(rejection) => {
                let actual_reason = &rejection.payload().guardian_rejection_reason;
                let actual = format!("Reject({actual_reason:?})");
                match &self.expected {
                    EvalExpect::Reject(expected_reason) => {
                        (true, actual_reason == expected_reason, actual)
                    }
                    EvalExpect::Accept => (false, false, actual),
                }
            }
            other => (false, false, format!("{other:?}")),
        }
    }

    fn expected_label(&self) -> String {
        match &self.expected {
            EvalExpect::Accept => "Accept".to_owned(),
            EvalExpect::Reject(reason) => format!("Reject({reason:?})"),
        }
    }
}

#[test]
#[ignore = "live DeepSeek eval; run explicitly to compare flash vs pro and tune the guardian"]
fn eval_flash_vs_pro_guardian() {
    if !DeepSeekKey::available() {
        eprintln!("skipping: DeepSeek gopass key unavailable");
        return;
    }
    let cases = eval_cases();
    let server = LiveAgentServer::spawn(cases.len() * EVAL_MODELS.len() * 2);

    for model in EVAL_MODELS {
        let mut verdict_hits = 0usize;
        let mut reason_hits = 0usize;
        let mut reason_total = 0usize;
        println!("\n===== GUARDIAN EVAL — {model} =====");
        for case in &cases {
            let directory = TempDir::new().expect("eval tempdir");
            let store = Store::open(directory.path().join("eval.sema")).expect("open eval store");
            let mut engine = Engine::new(store);
            for seed in seed_records() {
                let _ = engine.handle(Input::record(record_request(seed)));
            }
            engine.set_guardian(server.guardian_with_model(model));
            let proposal = Proposal {
                entry: case.entry.clone(),
                justification: eval_justification(case.quotes, case.reasoning),
            };
            let output = engine.handle(Input::propose(proposal));
            let (verdict_ok, reason_ok, actual) = case.score(output.root());
            if verdict_ok {
                verdict_hits += 1;
            }
            if let EvalExpect::Reject(_) = case.expected {
                reason_total += 1;
                if reason_ok {
                    reason_hits += 1;
                }
            }
            println!(
                "  [{verdict}{reason}] {name}: expected {expected}, got {actual}",
                verdict = if verdict_ok { 'V' } else { '.' },
                reason = if reason_ok { 'R' } else { ' ' },
                name = case.name,
                expected = case.expected_label(),
            );
        }
        println!(
            "  -> {model}: verdict {verdict_hits}/{total}, reason {reason_hits}/{reason_total}",
            total = cases.len(),
        );
    }
    server.join();
}

fn eval_entry(
    domains: &[&str],
    kind: Kind,
    certainty: Magnitude,
    importance: Magnitude,
    description: &str,
) -> Entry {
    Entry {
        domains: Domains::from_strings(domains.iter().map(|domain| (*domain).to_owned()).collect()),
        kind,
        description: Description::new(description),
        certainty: certainty.into(),
        importance: Importance::new(importance),
        privacy: Privacy::new(Magnitude::Zero),
        referents: Referents::new(Vec::new()),
    }
}

fn eval_justification(
    quotes: &[(&str, Option<&str>)],
    reasoning: &str,
) -> spirit::schema::signal::Justification {
    spirit::schema::signal::Justification {
        testimony: Testimony::new(
            quotes
                .iter()
                .map(|(quote, antecedent)| {
                    VerbatimQuote::new(
                        QuoteText::new((*quote).to_owned()),
                        antecedent.map(|text| Antecedent::new(text.to_owned())),
                    )
                })
                .collect(),
        ),
        reasoning: Reasoning::new(reasoning.to_owned()),
    }
}

fn eval_cases() -> Vec<EvalCase> {
    vec![
        EvalCase {
            name: "honest-low-accept",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Decision,
                Magnitude::Low,
                Magnitude::Minimum,
                "Guardian tests could use sandbox stores with realistic accept and reject proposals.",
            ),
            quotes: &[(
                "I think we could use a sandbox store for the guardian tests",
                None,
            )],
            reasoning: "A tentative lean, recorded at Low certainty to match the hedge.",
            expected: EvalExpect::Accept,
        },
        EvalCase {
            name: "overclaim-high-reject",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Decision,
                Magnitude::High,
                Magnitude::Minimum,
                "Guardian tests use sandbox stores with realistic accept and reject proposals.",
            ),
            quotes: &[(
                "I think we could use a sandbox store for the guardian tests",
                None,
            )],
            reasoning: "Recorded firmly at High certainty.",
            expected: EvalExpect::Reject(GuardianRejectionReason::Overstated),
        },
        EvalCase {
            name: "firm-high-accept",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Decision,
                Magnitude::High,
                Magnitude::Minimum,
                "The meta-signal Import is the guardian-bypassing path for id-preserving restores.",
            ),
            quotes: &[(
                "we are going with the meta-signal Import as the guardian bypass, that is the rule",
                None,
            )],
            reasoning: "A flat commitment that clears High.",
            expected: EvalExpect::Accept,
        },
        EvalCase {
            name: "overclaim-maximum-reject",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Decision,
                Magnitude::Maximum,
                Magnitude::Minimum,
                "The meta-signal Import is the guardian-bypassing path for id-preserving restores.",
            ),
            quotes: &[(
                "we are going with the meta-signal Import as the guardian bypass, that is the rule",
                None,
            )],
            reasoning: "Recorded at Maximum certainty.",
            expected: EvalExpect::Reject(GuardianRejectionReason::Overstated),
        },
        EvalCase {
            name: "missing-testimony-reject",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Decision,
                Magnitude::Medium,
                Magnitude::Minimum,
                "The guardian decision journal stays separate from the live intent store.",
            ),
            quotes: &[],
            reasoning: "The team aligned on keeping the journal separate.",
            expected: EvalExpect::Reject(GuardianRejectionReason::MissingTestimony),
        },
        EvalCase {
            name: "orthogonal-axes-accept",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Decision,
                Magnitude::VeryLow,
                Magnitude::High,
                "Whether the guardian should be one model or two is still unresolved.",
            ),
            quotes: &[(
                "I keep coming back to whether the guardian should be one model or two and I am really not sure yet",
                None,
            )],
            reasoning: "The topic recurs across three sessions and blocks the guardian design, so importance is High while certainty stays VeryLow.",
            expected: EvalExpect::Accept,
        },
        EvalCase {
            name: "importance-unsupported-reject",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Decision,
                Magnitude::VeryLow,
                Magnitude::High,
                "Running two guardian models in parallel might be interesting.",
            ),
            quotes: &[("maybe two guardian models could be interesting", None)],
            reasoning: "This is high importance.",
            expected: EvalExpect::Reject(GuardianRejectionReason::ImportanceUnsupported),
        },
        EvalCase {
            name: "compound-reject",
            entry: eval_entry(
                &["software", "agent"],
                Kind::Decision,
                Magnitude::High,
                Magnitude::Minimum,
                "Agent resolves DeepSeek keys through gopass and Spirit deploys the guardian immediately.",
            ),
            quotes: &[(
                "resolve the keys through gopass and also deploy the guardian right away",
                None,
            )],
            reasoning: "Two separable arrows in one record.",
            expected: EvalExpect::Reject(GuardianRejectionReason::Compound),
        },
        EvalCase {
            name: "non-intent-reject",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Decision,
                Magnitude::Medium,
                Magnitude::Minimum,
                "The corpus rebuild may not be ready yet.",
            ),
            quotes: &[(
                "I am not sure whether the rebuild is ready, let me look again",
                None,
            )],
            reasoning: "A momentary status check.",
            expected: EvalExpect::Reject(GuardianRejectionReason::NonIntent),
        },
        EvalCase {
            name: "negative-guideline-reject",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Correction,
                Magnitude::Medium,
                Magnitude::Minimum,
                "Canonical prose names are criome for the authentication component and criomos for the operating system name; creome and creomos are misspellings.",
            ),
            quotes: &[("its criome and criomos, not creome and creomos", None)],
            reasoning: "This centers the rejected spellings rather than pleading only the affirmative canonical names.",
            expected: EvalExpect::Reject(GuardianRejectionReason::NegativeGuideline),
        },
        EvalCase {
            name: "duplicate-reject",
            entry: eval_entry(
                &["software", "agent"],
                Kind::Decision,
                Magnitude::High,
                Magnitude::Minimum,
                "Agent provider secrets are resolved by the agent daemon from configured secret-source backends.",
            ),
            quotes: &[(
                "agent secrets come from the configured backends, that is settled",
                None,
            )],
            reasoning: "Restates a settled rule about secret resolution.",
            expected: EvalExpect::Reject(GuardianRejectionReason::Duplicate),
        },
        EvalCase {
            name: "contradiction-reject",
            entry: eval_entry(
                &["software", "nota"],
                Kind::Decision,
                Magnitude::High,
                Magnitude::Minimum,
                "NOTA strings should use quotation marks as the canonical representation.",
            ),
            quotes: &[(
                "let us make quotation marks the canonical NOTA string form",
                None,
            )],
            reasoning: "Proposes the quotation-mark form firmly.",
            expected: EvalExpect::Reject(GuardianRejectionReason::Contradiction),
        },
        EvalCase {
            name: "bare-yes-with-antecedent-accept",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Decision,
                Magnitude::High,
                Magnitude::Minimum,
                "The daemon rejects inline NOTA configuration.",
            ),
            quotes: &[(
                "yes do that",
                Some("shall we make the daemon reject inline NOTA configuration"),
            )],
            reasoning: "The psyche affirmed the antecedent question, which carries the arrow.",
            expected: EvalExpect::Accept,
        },
        EvalCase {
            name: "bare-yes-no-antecedent-reject",
            entry: eval_entry(
                &["software", "spirit"],
                Kind::Decision,
                Magnitude::High,
                Magnitude::Minimum,
                "The daemon rejects inline NOTA configuration.",
            ),
            quotes: &[("yes do that", None)],
            reasoning: "An affirmation with no antecedent.",
            expected: EvalExpect::Reject(GuardianRejectionReason::MissingTestimony),
        },
    ]
}
