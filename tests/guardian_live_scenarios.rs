#![cfg(feature = "agent-guardian")]

mod support;

use support::domain_fixtures;

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
use meta_signal_spirit::schema::meta_signal::{ImportRequest, ImportedRecord, ImportedRecords};
use signal_agent::Input as AgentInput;
use spirit::{
    AgentGuardian, AgentGuardianConfiguration, Engine, Store,
    schema::signal::{
        Antecedent, Description, Entry, GuardianRejectionReason, Importance, Input, Kind,
        Magnitude, Output, Privacy, Proposal, QuoteText, Reasoning, RecordIdentifier, Referent,
        Referents, Testimony, VerbatimQuote,
    },
};
use tempfile::TempDir;
use triad_runtime::{FrameBody, LengthPrefixedCodec};

const DEEPSEEK_PROVIDER: &str = "deepseek";
const DEEPSEEK_ENDPOINT: &str = "https://api.deepseek.com/v1";
const DEEPSEEK_MODEL: &str = "deepseek-v4-flash";
const DEEPSEEK_GOPASS_PATH: &str = "platform.deepseek.com/api-key";
const JUDGE_LIVE_EVAL_FIXTURE: &str = include_str!("fixtures/spirit_judge_live_eval.nota");

#[derive(Clone)]
enum LiveAgentProviderConfig {
    DeepSeek,
    LocalOpenAiCompatible,
}

impl LiveAgentProviderConfig {
    fn provider_name(&self) -> &'static str {
        match self {
            Self::DeepSeek => DEEPSEEK_PROVIDER,
            Self::LocalOpenAiCompatible => {
                AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_PROVIDER
            }
        }
    }

    fn endpoint(&self) -> &'static str {
        match self {
            Self::DeepSeek => DEEPSEEK_ENDPOINT,
            Self::LocalOpenAiCompatible => {
                AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_ENDPOINT
            }
        }
    }

    fn model_name(&self) -> &'static str {
        match self {
            Self::DeepSeek => DEEPSEEK_MODEL,
            Self::LocalOpenAiCompatible => {
                AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_MODEL
            }
        }
    }

    fn secret_source(&self) -> SecretSource {
        match self {
            Self::DeepSeek => SecretSource::gopass(DEEPSEEK_GOPASS_PATH),
            Self::LocalOpenAiCompatible => SecretSource::no_secret(),
        }
    }
}

struct LiveAgentServer {
    _directory: TempDir,
    socket_path: std::path::PathBuf,
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

struct GuardianScenario {
    name: String,
    proposal: Entry,
    quote: String,
    reasoning: String,
    expected: ExpectedVerdict,
}

enum ExpectedVerdict {
    Accept,
    Reject(Vec<GuardianRejectionReason>),
}

struct FixtureCorpus {
    seeds: Vec<ImportedRecord>,
    scenarios: Vec<GuardianScenario>,
}

struct FixtureLine<'fixture> {
    line_number: usize,
    text: &'fixture str,
}

impl FixtureCorpus {
    fn parse(text: &'static str) -> Self {
        let mut seeds = Vec::new();
        let mut scenarios = Vec::new();
        for (index, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fixture_line = FixtureLine {
                line_number: index + 1,
                text: line,
            };
            if let Some(rest) = line.strip_prefix("seed ") {
                seeds.push(fixture_line.seed_record(rest));
            } else if let Some(rest) = line.strip_prefix("case ") {
                scenarios.push(fixture_line.scenario(rest));
            } else {
                panic!("fixture line {} must start with seed or case", index + 1);
            }
        }
        assert!(
            seeds.len() >= 12,
            "fixture should preload a large enough neighbor corpus"
        );
        assert!(
            scenarios.len() >= 18,
            "fixture should cover broad judge cases"
        );
        Self { seeds, scenarios }
    }

    fn prepopulate(&self, engine: &mut Engine) {
        let output = engine.import(ImportRequest::new(ImportedRecords::new(self.seeds.clone())));
        assert!(
            matches!(
                output,
                meta_signal_spirit::schema::meta_signal::Output::Imported(_)
            ),
            "fixture import should bypass the judge through the owner-only typed Import path: {output:?}"
        );
    }
}

impl FixtureLine<'_> {
    fn seed_record(&self, rest: &str) -> ImportedRecord {
        let (identifier, input_text) =
            self.split_once(rest, ' ', "seed identifier and Record input");
        let input = self.input(input_text);
        let Input::Record(record) = input else {
            panic!(
                "fixture line {} seed must contain Record input",
                self.line_number
            );
        };
        ImportedRecord {
            record_identifier: RecordIdentifier::new(identifier),
            entry: record.into_payload().entry,
        }
    }

    fn scenario(&self, rest: &str) -> GuardianScenario {
        let (name, after_name) = self.split_once(rest, ' ', "case name");
        let (expected_text, input_text) = self.split_once(after_name, ' ', "case expected verdict");
        let input = self.input(input_text);
        let Input::Propose(propose) = input else {
            panic!(
                "fixture line {} case must contain Propose input",
                self.line_number
            );
        };
        let proposal = propose.into_payload();
        let quote = proposal
            .justification
            .testimony
            .payload()
            .first()
            .map(|quote| quote.quote_text.payload().as_str())
            .unwrap_or("");
        let reasoning = proposal.justification.reasoning.payload().as_str();
        GuardianScenario {
            name: name.to_owned(),
            proposal: proposal.entry,
            quote: quote.to_owned(),
            reasoning: reasoning.to_owned(),
            expected: self.expected(expected_text),
        }
    }

    fn input(&self, input_text: &str) -> Input {
        input_text.parse::<Input>().unwrap_or_else(|error| {
            panic!(
                "fixture line {} failed generated Input parse: {error}; text: {input_text}",
                self.line_number
            )
        })
    }

    fn expected(&self, text: &str) -> ExpectedVerdict {
        if text == "Accept" {
            return ExpectedVerdict::Accept;
        }
        let Some(reasons) = text.strip_prefix("Reject:") else {
            panic!(
                "fixture line {} has invalid expected verdict {text}",
                self.line_number
            );
        };
        ExpectedVerdict::Reject(
            reasons
                .split(',')
                .map(|reason| self.rejection_reason(reason))
                .collect(),
        )
    }

    fn rejection_reason(&self, reason: &str) -> GuardianRejectionReason {
        match reason {
            "MissingTestimony" => GuardianRejectionReason::MissingTestimony,
            "TestimonyFabricated" => GuardianRejectionReason::TestimonyFabricated,
            "InsufficientWarrant" => GuardianRejectionReason::InsufficientWarrant,
            "Overstated" => GuardianRejectionReason::Overstated,
            "ImportanceUnsupported" => GuardianRejectionReason::ImportanceUnsupported,
            "NonIntent" => GuardianRejectionReason::NonIntent,
            "NegativeGuideline" => GuardianRejectionReason::NegativeGuideline,
            "Matter" => GuardianRejectionReason::Matter,
            "Compound" => GuardianRejectionReason::Compound,
            "UnclearDomain" => GuardianRejectionReason::UnclearDomain,
            "UnclearPrivacy" => GuardianRejectionReason::UnclearPrivacy,
            "Duplicate" => GuardianRejectionReason::Duplicate,
            "Contradiction" => GuardianRejectionReason::Contradiction,
            "ClarifyTramples" => GuardianRejectionReason::ClarifyTramples,
            "ClarifyLosesMeaning" => GuardianRejectionReason::ClarifyLosesMeaning,
            "SupersedeTargetMissing" => GuardianRejectionReason::SupersedeTargetMissing,
            "RetrievalInsufficient" => GuardianRejectionReason::RetrievalInsufficient,
            other => panic!(
                "fixture line {} has unknown rejection reason {other}",
                self.line_number
            ),
        }
    }

    fn split_once<'text>(
        &self,
        text: &'text str,
        delimiter: char,
        expectation: &str,
    ) -> (&'text str, &'text str) {
        text.split_once(delimiter).unwrap_or_else(|| {
            panic!(
                "fixture line {} missing {expectation}: {}",
                self.line_number, self.text
            )
        })
    }
}

impl LiveAgentServer {
    fn spawn(provider_config: LiveAgentProviderConfig, call_count: usize) -> Self {
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
                provider_config.provider_name(),
                provider_config.endpoint(),
                provider_config.model_name(),
                provider_config.secret_source(),
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

    fn guardian(&self, provider_config: LiveAgentProviderConfig) -> AgentGuardian {
        AgentGuardian::new(AgentGuardianConfiguration::new(
            self.socket_path.clone(),
            Some(provider_config.provider_name().to_owned()),
            Some(provider_config.model_name().to_owned()),
            Duration::from_secs(120),
            None,
        ))
    }

    fn join(self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().expect("live agent server joins");
    }
}

fn proposal(entry: Entry, quote: &str, reasoning: &str) -> Proposal {
    Proposal {
        entry,
        justification: eval_justification(&[(quote, None)], reasoning),
    }
}

#[test]
fn judge_live_eval_fixture_loads_and_imports_seed_corpus() {
    let corpus = FixtureCorpus::parse(JUDGE_LIVE_EVAL_FIXTURE);
    let directory = TempDir::new().expect("spirit fixture tempdir");
    let store = Store::open(directory.path().join("fixture-prepopulation.sema"))
        .expect("open fixture store");
    let mut engine = Engine::new(store);
    corpus.prepopulate(&mut engine);
    assert_eq!(
        corpus.seeds.len(),
        14,
        "fixture seed count should stay intentional"
    );
    assert_eq!(
        corpus.scenarios.len(),
        21,
        "fixture scenario count should stay intentional"
    );
}

#[test]
#[ignore = "uses live DeepSeek through gopass; run explicitly before judge deployment"]
fn live_deepseek_guardian_accepts_and_rejects_realistic_scenarios() {
    if !DeepSeekKey::available() {
        eprintln!("skipping: DeepSeek gopass key unavailable");
        return;
    }
    live_guardian_accepts_and_rejects_realistic_scenarios(LiveAgentProviderConfig::DeepSeek);
}

#[test]
#[ignore = "uses a local OpenAI-compatible endpoint at http://127.0.0.1:18080/v1 with the Mind-verified gpt-5.5 model"]
fn live_local_openai_compatible_guardian_accepts_and_rejects_realistic_scenarios() {
    if !LocalOpenAiCompatibleEndpoint::available() {
        eprintln!(
            "skipping: local OpenAI-compatible endpoint unavailable at {} for model {}",
            AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_ENDPOINT,
            AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_MODEL
        );
        return;
    }
    live_guardian_accepts_and_rejects_realistic_scenarios(
        LiveAgentProviderConfig::LocalOpenAiCompatible,
    );
}

fn live_guardian_accepts_and_rejects_realistic_scenarios(provider_config: LiveAgentProviderConfig) {
    let corpus = FixtureCorpus::parse(JUDGE_LIVE_EVAL_FIXTURE);
    let directory = TempDir::new().expect("spirit tempdir");
    let store = Store::open(directory.path().join("guardian-live.sema")).expect("open store");
    let mut engine = Engine::new(store);
    corpus.prepopulate(&mut engine);

    let live_agent = LiveAgentServer::spawn(provider_config.clone(), corpus.scenarios.len() * 2);
    engine.set_guardian(live_agent.guardian(provider_config));

    for scenario in corpus.scenarios {
        let output = engine.handle(Input::propose(proposal(
            scenario.proposal,
            &scenario.quote,
            &scenario.reasoning,
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

struct LocalOpenAiCompatibleEndpoint;

impl LocalOpenAiCompatibleEndpoint {
    fn available() -> bool {
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], 18080)),
            Duration::from_millis(250),
        )
        .is_ok()
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
    let corpus = FixtureCorpus::parse(JUDGE_LIVE_EVAL_FIXTURE);
    let server = LiveAgentServer::spawn(
        LiveAgentProviderConfig::DeepSeek,
        cases.len() * EVAL_MODELS.len() * 2,
    );

    for model in EVAL_MODELS {
        let mut verdict_hits = 0usize;
        let mut reason_hits = 0usize;
        let mut reason_total = 0usize;
        println!("\n===== GUARDIAN EVAL — {model} =====");
        for case in &cases {
            let directory = TempDir::new().expect("eval tempdir");
            let store = Store::open(directory.path().join("eval.sema")).expect("open eval store");
            let mut engine = Engine::new(store);
            corpus.prepopulate(&mut engine);
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
        domains: domain_fixtures::domains(domains),
        kind,
        description: Description::new(description),
        certainty: certainty.into(),
        importance: Importance::new(importance),
        privacy: Privacy::new(Magnitude::Zero),
        referents: Referents::new(vec![Referent::new("spirit")]),
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
