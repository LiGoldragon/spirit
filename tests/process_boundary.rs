mod support;

use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, ChildStdout, Command, Stdio},
    str::FromStr,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};
use support::domain_fixtures;

use nota::NotaEncode;
#[cfg(feature = "testing-trace")]
use nota::NotaSource;
#[cfg(feature = "agent-guardian")]
use signal_agent::{
    Completion, CompletionText, Input as AgentInput, Output as AgentOutput, ReasoningEffort,
    StopReasonText, ThinkingMode, TokenUsage,
};
#[cfg(feature = "testing-trace")]
use signal_introspect::ComponentTraceEvent;
#[cfg(feature = "agent-guardian")]
use signal_spirit::{
    ConfigurationPath, SpiritGuardianAgentConfiguration, SpiritGuardianTimeoutMilliseconds,
};
use spirit::Configuration;
#[cfg(feature = "agent-guardian")]
use spirit::schema::nexus::GuardianVerdict;
use spirit::schema::signal::{
    Antecedent, ClarificationRecordIdentifier, ClarificationResolution, Description, Input,
    IntentEvent, Justification, Kind, Magnitude, Output, QuoteText, Reasoning, RecordIdentifier,
    TargetClarification, TargetClarifications, Testimony, VerbatimQuote,
};
#[cfg(feature = "agent-guardian")]
use std::{
    io::Write,
    os::unix::net::{UnixListener, UnixStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tempfile::TempDir;
#[cfg(feature = "agent-guardian")]
use triad_runtime::{FrameBody, LengthPrefixedCodec};

struct DaemonProcess {
    child: Child,
    #[cfg(feature = "agent-guardian")]
    _guardian_agent: FakeGuardianAgent,
}

#[cfg(feature = "agent-guardian")]
struct FakeGuardianAgent {
    socket_path: std::path::PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

struct SubscriberProcess {
    child: Child,
    lines: Receiver<String>,
    reader_thread: Option<thread::JoinHandle<()>>,
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(feature = "agent-guardian")]
impl Drop for FakeGuardianAgent {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for SubscriberProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
    }
}

#[cfg(feature = "agent-guardian")]
impl FakeGuardianAgent {
    fn spawn(socket_path: std::path::PathBuf) -> Self {
        let listener = UnixListener::bind(&socket_path).expect("bind fake guardian socket");
        listener
            .set_nonblocking(true)
            .expect("fake guardian listener nonblocking");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => Self::answer(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept fake guardian call: {error}"),
                }
            }
        });
        Self {
            socket_path,
            stop,
            thread: Some(thread),
        }
    }

    fn answer(mut stream: UnixStream) {
        let codec = LengthPrefixedCodec::default();
        let request = codec
            .read_body(&mut stream)
            .expect("read fake guardian request")
            .into_bytes();
        let (_route, input) =
            AgentInput::decode_signal_frame(&request).expect("decode fake guardian input");
        let AgentInput::Call(call) = input else {
            panic!("expected guardian Call input, got {input:?}");
        };
        let options = call.payload().prompt_options();
        assert_eq!(
            options
                .provider()
                .map(|provider| provider.payload().as_str()),
            Some(spirit::AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_PROVIDER),
            "omitted daemon guardian provider resolves through the agent-daemon call to local-openai"
        );
        assert_eq!(
            options.model().map(|model| model.payload().as_str()),
            Some(spirit::AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_MODEL),
            "omitted daemon guardian model resolves through the agent-daemon call to gpt-5.4-mini"
        );
        assert_eq!(options.reasoning_effort(), Some(&ReasoningEffort::Medium));
        assert_eq!(options.thinking_mode(), None::<&ThinkingMode>);
        let output = AgentOutput::completed(Completion {
            completion_text: CompletionText::new(GuardianVerdict::Accept.to_nota()),
            stop_reason_text: StopReasonText::new("stop"),
            token_usage: TokenUsage::new(None, None),
        });
        codec
            .write_body(
                &mut stream,
                &FrameBody::new(
                    output
                        .encode_signal_frame()
                        .expect("encode fake guardian output"),
                ),
            )
            .expect("write fake guardian output");
        stream.flush().expect("flush fake guardian output");
    }

    fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl DaemonProcess {
    fn meta_socket_path(socket_path: &Path) -> std::path::PathBuf {
        socket_path.with_extension("meta.sock")
    }

    #[cfg(feature = "agent-guardian")]
    fn guardian_agent(socket_path: &Path) -> FakeGuardianAgent {
        FakeGuardianAgent::spawn(socket_path.with_extension("agent.sock"))
    }

    #[cfg(feature = "agent-guardian")]
    fn configuration_with_guardian(
        configuration: Configuration,
        guardian_agent: &FakeGuardianAgent,
    ) -> Configuration {
        Configuration::from_raw(
            configuration
                .raw()
                .clone()
                .with_guardian_agent_configuration(SpiritGuardianAgentConfiguration::new(
                    ConfigurationPath::new(
                        guardian_agent.socket_path().to_string_lossy().into_owned(),
                    ),
                    None,
                    None,
                    SpiritGuardianTimeoutMilliseconds::new(5_000),
                    None,
                )),
        )
    }

    fn spawn(socket_path: &Path, database_path: &Path) -> Self {
        let configuration_path = socket_path.with_extension("config.rkyv");
        let meta_socket_path = Self::meta_socket_path(socket_path);
        #[cfg(feature = "agent-guardian")]
        let guardian_agent = Self::guardian_agent(socket_path);
        let configuration =
            Configuration::new(socket_path, database_path).with_meta_socket_path(&meta_socket_path);
        #[cfg(feature = "agent-guardian")]
        let configuration = Self::configuration_with_guardian(configuration, &guardian_agent);
        configuration
            .write_binary_file(&configuration_path)
            .expect("write binary daemon configuration");
        let child = Command::new(env!("CARGO_BIN_EXE_spirit-daemon"))
            .arg(configuration_path)
            .spawn()
            .expect("spawn daemon");
        let process = Self {
            child,
            #[cfg(feature = "agent-guardian")]
            _guardian_agent: guardian_agent,
        };
        wait_for_socket(socket_path);
        wait_for_socket(&meta_socket_path);
        process
    }

    fn spawn_from_configuration_writer(socket_path: &Path, database_path: &Path) -> Self {
        let configuration_path = socket_path.with_extension("config.rkyv");
        let meta_socket_path = Self::meta_socket_path(socket_path);
        #[cfg(feature = "agent-guardian")]
        let guardian_agent = Self::guardian_agent(socket_path);
        #[cfg(not(feature = "agent-guardian"))]
        let request = format!(
            "(ConfigurationWriteRequest ({} (Some {}) {} None Gating None {}))",
            nota_path(socket_path),
            nota_path(&meta_socket_path),
            nota_path(database_path),
            nota_path(&configuration_path)
        );
        #[cfg(feature = "agent-guardian")]
        let request = format!(
            "(ConfigurationWriteRequest ({} (Some {}) {} None Gating (Some ({} None None 5000 None)) {}))",
            nota_path(socket_path),
            nota_path(&meta_socket_path),
            nota_path(database_path),
            nota_path(guardian_agent.socket_path()),
            nota_path(&configuration_path)
        );
        let output = Command::new(env!("CARGO_BIN_EXE_spirit-write-configuration"))
            .arg(request)
            .output()
            .expect("run configuration writer");
        assert!(
            output.status.success(),
            "configuration writer stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout =
            String::from_utf8(output.stdout).expect("configuration writer stdout is UTF-8");
        assert_eq!(
            stdout.trim(),
            format!("(ConfigurationWritten {})", nota_path(&configuration_path))
        );
        let child = Command::new(env!("CARGO_BIN_EXE_spirit-daemon"))
            .arg(configuration_path)
            .spawn()
            .expect("spawn daemon from writer-built configuration");
        let process = Self {
            child,
            #[cfg(feature = "agent-guardian")]
            _guardian_agent: guardian_agent,
        };
        wait_for_socket(socket_path);
        wait_for_socket(&meta_socket_path);
        process
    }

    #[cfg(feature = "testing-trace")]
    fn spawn_with_trace(
        socket_path: &Path,
        database_path: &Path,
        trace_socket_path: &Path,
    ) -> Self {
        let configuration_path = socket_path.with_extension("config.rkyv");
        let meta_socket_path = Self::meta_socket_path(socket_path);
        #[cfg(feature = "agent-guardian")]
        let guardian_agent = Self::guardian_agent(socket_path);
        let configuration =
            Configuration::new_with_trace(socket_path, database_path, trace_socket_path)
                .with_meta_socket_path(&meta_socket_path);
        #[cfg(feature = "agent-guardian")]
        let configuration = Self::configuration_with_guardian(configuration, &guardian_agent);
        configuration
            .write_binary_file(&configuration_path)
            .expect("write binary daemon configuration with trace socket");
        let child = Command::new(env!("CARGO_BIN_EXE_spirit-daemon"))
            .arg(configuration_path)
            .spawn()
            .expect("spawn daemon");
        let process = Self {
            child,
            #[cfg(feature = "agent-guardian")]
            _guardian_agent: guardian_agent,
        };
        wait_for_socket(socket_path);
        wait_for_socket(&meta_socket_path);
        process
    }
}

#[test]
fn configuration_writer_accepts_guardian_without_output_budget() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("spirit.sock");
    let meta_socket_path = directory.path().join("meta.sock");
    let database_path = directory.path().join("spirit.sema");
    let agent_socket_path = directory.path().join("agent.sock");
    let configuration_path = directory.path().join("spirit.config.rkyv");
    let request = format!(
        "(ConfigurationWriteRequest ({} (Some {}) {} None Gating (Some ({} (Some {}) (Some {}) 120000 None)) {}))",
        nota_path(&socket_path),
        nota_path(&meta_socket_path),
        nota_path(&database_path),
        nota_path(&agent_socket_path),
        String::from("deepseek").to_nota(),
        String::from("deepseek-v4-flash").to_nota(),
        nota_path(&configuration_path)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_spirit-write-configuration"))
        .arg(request)
        .output()
        .expect("run configuration writer");

    assert!(
        output.status.success(),
        "configuration writer stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let configuration =
        Configuration::from_binary_path(&configuration_path).expect("decode binary config");
    let guardian = configuration
        .raw()
        .guardian_agent_configuration()
        .expect("guardian configuration");
    assert_eq!(
        guardian.agent_socket_path(),
        agent_socket_path.to_string_lossy()
    );
    assert_eq!(guardian.provider_name(), Some("deepseek"));
    assert_eq!(guardian.model_name(), Some("deepseek-v4-flash"));
    assert_eq!(guardian.timeout_milliseconds(), 120_000);
    assert_eq!(guardian.maximum_output_tokens(), None);
}

#[test]
fn configuration_writer_omitted_guardian_provider_resolves_to_local_openai_compatible_judge() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("spirit.sock");
    let meta_socket_path = directory.path().join("meta.sock");
    let database_path = directory.path().join("spirit.sema");
    let agent_socket_path = directory.path().join("agent.sock");
    let configuration_path = directory.path().join("spirit.config.rkyv");
    let request = format!(
        "(ConfigurationWriteRequest ({} (Some {}) {} None Gating (Some ({} None None 180000 None)) {}))",
        nota_path(&socket_path),
        nota_path(&meta_socket_path),
        nota_path(&database_path),
        nota_path(&agent_socket_path),
        nota_path(&configuration_path)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_spirit-write-configuration"))
        .arg(request)
        .output()
        .expect("run configuration writer");

    assert!(
        output.status.success(),
        "configuration writer stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let configuration =
        Configuration::from_binary_path(&configuration_path).expect("decode binary config");
    let raw_guardian = configuration
        .raw()
        .guardian_agent_configuration()
        .expect("raw guardian configuration");
    assert_eq!(raw_guardian.provider_name(), None);
    assert_eq!(raw_guardian.model_name(), None);
    let judge = configuration
        .guardian_agent_configuration()
        .expect("resolved judge configuration");
    assert_eq!(
        judge.provider_name(),
        Some(spirit::AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_PROVIDER)
    );
    assert_eq!(
        judge.model_name(),
        Some(spirit::AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_MODEL)
    );
}

#[test]
fn configuration_writer_accepts_local_openai_compatible_guardian() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("spirit.sock");
    let meta_socket_path = directory.path().join("meta.sock");
    let database_path = directory.path().join("spirit.sema");
    let agent_socket_path = directory.path().join("agent.sock");
    let configuration_path = directory.path().join("spirit.config.rkyv");
    let request = format!(
        "(ConfigurationWriteRequest ({} (Some {}) {} None Gating (Some ({} (Some {}) (Some {}) 180000 None)) {}))",
        nota_path(&socket_path),
        nota_path(&meta_socket_path),
        nota_path(&database_path),
        nota_path(&agent_socket_path),
        String::from(spirit::AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_PROVIDER)
            .to_nota(),
        String::from(spirit::AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_MODEL).to_nota(),
        nota_path(&configuration_path)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_spirit-write-configuration"))
        .arg(request)
        .output()
        .expect("run configuration writer");

    assert!(
        output.status.success(),
        "configuration writer stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let configuration =
        Configuration::from_binary_path(&configuration_path).expect("decode binary config");
    let guardian = configuration
        .raw()
        .guardian_agent_configuration()
        .expect("guardian configuration");
    assert_eq!(
        guardian.agent_socket_path(),
        agent_socket_path.to_string_lossy()
    );
    assert_eq!(
        guardian.provider_name(),
        Some(spirit::AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_PROVIDER)
    );
    assert_eq!(
        guardian.model_name(),
        Some(spirit::AgentGuardianConfiguration::LOCAL_OPENAI_COMPATIBLE_MODEL)
    );
    assert_eq!(guardian.timeout_milliseconds(), 180_000);
    assert_eq!(guardian.maximum_output_tokens(), None);
}

#[test]
fn configuration_writer_encodes_observing_authorization_mode() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("spirit.sock");
    let meta_socket_path = directory.path().join("meta.sock");
    let database_path = directory.path().join("spirit.sema");
    let configuration_path = directory.path().join("spirit.config.rkyv");
    let request = format!(
        "(ConfigurationWriteRequest ({} (Some {}) {} None Observing None {}))",
        nota_path(&socket_path),
        nota_path(&meta_socket_path),
        nota_path(&database_path),
        nota_path(&configuration_path)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_spirit-write-configuration"))
        .arg(request)
        .output()
        .expect("run configuration writer");
    assert!(
        output.status.success(),
        "configuration writer succeeds: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let configuration =
        Configuration::from_binary_path(&configuration_path).expect("decode binary config");

    assert_eq!(
        configuration.authorization_mode(),
        signal_spirit::AuthorizationMode::Observing
    );
}

impl SubscriberProcess {
    fn spawn(socket_path: &Path, nota_argument: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_spirit"))
            .env("SPIRIT_SOCKET", socket_path)
            .arg(nota_argument)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn subscriber cli");
        let stdout = child.stdout.take().expect("subscriber stdout");
        let output = SubscriberOutput::new(stdout);
        Self {
            child,
            lines: output.lines,
            reader_thread: Some(output.reader_thread),
        }
    }

    fn next_output(&self, timeout: Duration) -> Output {
        let line = self
            .lines
            .recv_timeout(timeout)
            .expect("subscriber output before timeout");
        Output::from_str(line.trim()).unwrap_or_else(|error| {
            panic!("schema-emitted Output::FromStr on subscriber stdout {line:?}: {error}")
        })
    }

    fn assert_no_output(&self, timeout: Duration) {
        match self.lines.recv_timeout(timeout) {
            Ok(line) => panic!("subscriber should not receive output, got {line:?}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("subscriber exited before timeout")
            }
        }
    }
}

struct SubscriberOutput {
    lines: Receiver<String>,
    reader_thread: thread::JoinHandle<()>,
}

impl SubscriberOutput {
    fn new(stdout: ChildStdout) -> Self {
        let (sender, lines) = mpsc::channel();
        let reader_thread = thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            lines,
            reader_thread,
        }
    }
}

fn run_cli(socket_path: &Path, nota_argument: &str) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_spirit"))
        .env("SPIRIT_SOCKET", socket_path)
        .arg(nota_argument)
        .output()
        .expect("run cli");
    assert!(
        output.status.success(),
        "cli stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("cli stdout is UTF-8");
    Output::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "schema-emitted Output::FromStr on CLI stdout {:?}: {error}",
            stdout.trim()
        )
    })
}

fn record_nota(domains: &str, kind: &str, description: &str) -> String {
    format!(
        "(Record (({domains} {kind} [{description}] Maximum Minimum Zero [process-boundary-test]) ([([{description}] None)] [{description}])))"
    )
}

fn resolve_clarification_nota(
    clarification_identifier: RecordIdentifier,
    target_identifier: RecordIdentifier,
    description: &str,
) -> String {
    Input::resolve_clarification(ClarificationResolution {
        clarification_record_identifier: ClarificationRecordIdentifier::new(
            clarification_identifier,
        ),
        target_clarifications: TargetClarifications::new(vec![TargetClarification {
            record_identifier: target_identifier,
            description: Description::new(description),
        }]),
        justification: test_justification("a clarification means edit the target, not add more"),
    })
    .to_nota()
}

fn assert_short_record_identifier(identifier: &RecordIdentifier) {
    let payload = identifier.payload();
    assert!(
        (4..=7).contains(&payload.len()),
        "record identifier should use a four-to-seven-character code: {payload}"
    );
    assert!(
        payload
            .chars()
            .all(|character| character.is_ascii_digit() || character.is_ascii_lowercase()),
        "record identifier should be lower-base36: {payload}"
    );
}

fn record_identifier_argument(identifier: &RecordIdentifier) -> String {
    identifier.payload().to_string()
}

fn test_justification(statement: &str) -> Justification {
    Justification {
        testimony: Testimony::new(vec![VerbatimQuote::new(
            QuoteText::new(statement),
            Some(Antecedent::new("test setup")),
        )]),
        reasoning: Reasoning::new(statement),
    }
}

fn nota_path(path: &Path) -> String {
    path.display().to_string().to_nota()
}

#[cfg(feature = "testing-trace")]
#[derive(Debug)]
struct TraceCliOutput {
    output: Output,
    trace_lines: Vec<String>,
}

#[cfg(feature = "testing-trace")]
impl TraceCliOutput {
    fn from_stdout(stdout: Vec<u8>) -> Self {
        let stdout = String::from_utf8(stdout).expect("cli stdout is UTF-8");
        let mut lines = stdout.lines();
        let output_line = lines.next().expect("cli prints signal output");
        let output = Output::from_str(output_line).unwrap_or_else(|error| {
            panic!("schema-emitted Output::FromStr on CLI stdout {output_line:?}: {error}")
        });
        Self {
            output,
            trace_lines: lines.map(String::from).collect(),
        }
    }

    fn assert_trace_sequence(&self, expected: &[&str]) {
        let events = self.trace_events();
        let actual = events
            .iter()
            .map(|event| event.event_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "trace lines: {:#?}", self.trace_lines);
    }

    fn assert_trace_sequence_after_optional_lifecycle_start(&self, expected: &[&str]) {
        let events = self.trace_events();
        let mut actual = events
            .iter()
            .map(|event| event.event_name.as_str())
            .collect::<Vec<_>>();
        let lifecycle_start = ["SemaStarted", "NexusStarted", "SignalStarted"];
        if actual.starts_with(&lifecycle_start) {
            actual.drain(..lifecycle_start.len());
        }
        assert_eq!(actual, expected, "trace lines: {:#?}", self.trace_lines);
    }

    fn trace_events(&self) -> Vec<ComponentTraceEvent> {
        self.trace_lines
            .iter()
            .map(|line| {
                let event = NotaSource::new(line)
                    .parse::<ComponentTraceEvent>()
                    .unwrap_or_else(|error| {
                        panic!("trace CLI line should be component-trace NOTA {line:?}: {error}")
                    });
                assert_eq!(
                    event.to_string(),
                    *line,
                    "trace CLI line should be canonical component-trace NOTA"
                );
                event
            })
            .collect()
    }
}

#[cfg(feature = "testing-trace")]
fn run_cli_with_trace(
    socket_path: &Path,
    trace_socket_path: &Path,
    nota_argument: &str,
) -> TraceCliOutput {
    let output = Command::new(env!("CARGO_BIN_EXE_spirit"))
        .env("SPIRIT_SOCKET", socket_path)
        .env("SPIRIT_TRACE_SOCKET", trace_socket_path)
        .arg(nota_argument)
        .output()
        .expect("run cli with trace");
    assert!(
        output.status.success(),
        "cli stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    TraceCliOutput::from_stdout(output.stdout)
}

#[test]
fn configuration_writer_prebuilds_binary_archive_for_daemon_startup() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("written.sock");
    let database_path = temp.path().join("written.sema");

    let _daemon = DaemonProcess::spawn_from_configuration_writer(&socket_path, &database_path);

    let recorded = run_cli(
        &socket_path,
        &record_nota(
            "[(Technology (Software (Operations Deployment)))]",
            "Constraint",
            "daemon starts from prebuilt archive",
        ),
    );
    match recorded {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected RecordAccepted from writer-started daemon, got {other:?}"),
    }
}

#[test]
fn cli_and_daemon_exchange_nota_over_rkyv_socket() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("spirit.sock");
    let database_path = temp.path().join("spirit.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    // Record path — parsed back into the schema-emitted Output, asserted
    // on the typed variant (not on the raw digest string, which is now a
    // real content hash).
    let recorded = run_cli(
        &socket_path,
        &record_nota(
            "[(Information Documentation)]",
            "Constraint",
            "schema creates the interface",
        ),
    );
    match recorded {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    };

    let observed = run_cli(
        &socket_path,
        "(Observe ((Full [(Information Documentation)]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );
    // Observe flows through Stash and returns both the recovery handle and
    // the observed records.
    assert!(
        matches!(observed, Output::RecordsStashed(_)),
        "the daemon stashes and returns the observed records, got {observed:?}"
    );

    let rejected = run_cli(
        &socket_path,
        &record_nota("[]", "Constraint", "schema rejects before SEMA"),
    );
    assert!(
        matches!(rejected, Output::Rejected(_)),
        "empty domain is rejected before SEMA, got {rejected:?}"
    );
}

#[test]
fn public_text_search_returns_direct_ranked_records() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("public-text-search.sock");
    let database_path = temp.path().join("public-text-search.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    let first = run_cli(
        &socket_path,
        &record_nota(
            "[(Technology (Software (Distributed ProtocolDesign)))]",
            "Decision",
            "router.node.cluster.criome is the endpoint naming example",
        ),
    );
    assert!(
        matches!(first, Output::RecordAccepted(_)),
        "expected first search fixture to record, got {first:?}"
    );
    let second = run_cli(
        &socket_path,
        &record_nota(
            "[(Technology (Software (Engineering Architecture)))]",
            "Decision",
            "Router owns the standardized routing protocol envelope",
        ),
    );
    assert!(
        matches!(second, Output::RecordAccepted(_)),
        "expected second search fixture to record, got {second:?}"
    );

    let suffix_results = run_cli(&socket_path, "(PublicTextSearch .criome)");
    let Output::RecordsObserved(suffix_records) = suffix_results else {
        panic!("PublicTextSearch should return direct records, got {suffix_results:?}");
    };
    assert_eq!(suffix_records.payload().payload().len(), 1);
    assert_eq!(
        suffix_records.payload().payload()[0]
            .entry
            .description
            .payload(),
        "router.node.cluster.criome is the endpoint naming example"
    );

    let phrase_results = run_cli(&socket_path, "(PublicTextSearch [routing protocol])");
    let Output::RecordsObserved(phrase_records) = phrase_results else {
        panic!("PublicTextSearch should return direct phrase records, got {phrase_results:?}");
    };
    assert_eq!(phrase_records.payload().payload().len(), 1);
    assert_eq!(
        phrase_records.payload().payload()[0]
            .entry
            .description
            .payload(),
        "Router owns the standardized routing protocol envelope"
    );
}

#[test]
fn cli_and_daemon_resolve_clarification_edits_target_and_removes_standalone() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("resolve-clarification.sock");
    let database_path = temp.path().join("resolve-clarification.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    let target = run_cli(
        &socket_path,
        &record_nota(
            "[(Information Documentation)]",
            "Decision",
            "clarifications should not add more records",
        ),
    );
    let target_identifier = match target {
        Output::RecordAccepted(receipt) => receipt.payload().clone(),
        other => panic!("expected target RecordAccepted, got {other:?}"),
    };

    let standalone = run_cli(
        &socket_path,
        &record_nota(
            "[(Information Documentation)]",
            "Clarification",
            "bad standalone clarification to fold away",
        ),
    );
    let clarification_identifier = match standalone {
        Output::RecordAccepted(receipt) => receipt.payload().clone(),
        other => panic!("expected clarification RecordAccepted, got {other:?}"),
    };

    let resolved = run_cli(
        &socket_path,
        &resolve_clarification_nota(
            clarification_identifier.clone(),
            target_identifier.clone(),
            "clarifications edit target records instead of adding more records",
        ),
    );
    match resolved {
        Output::ClarificationResolved(receipt) => {
            assert_eq!(
                receipt.payload().clarification_record_identifier.payload(),
                &clarification_identifier
            );
            assert_eq!(
                receipt.payload().record_identifiers.payload(),
                &vec![target_identifier.clone()]
            );
        }
        other => panic!("expected ClarificationResolved, got {other:?}"),
    }

    let found = run_cli(
        &socket_path,
        &format!(
            "(Lookup {})",
            record_identifier_argument(&target_identifier)
        ),
    );
    match found {
        Output::RecordFound(record) => {
            assert_eq!(record.record_identifier, target_identifier);
            assert_eq!(
                record.entry.description.payload(),
                "clarifications edit target records instead of adding more records"
            );
        }
        other => panic!("expected target RecordFound, got {other:?}"),
    }

    let missing = run_cli(
        &socket_path,
        &format!(
            "(Lookup {})",
            record_identifier_argument(&clarification_identifier)
        ),
    );
    assert!(
        matches!(missing, Output::Error(_)),
        "standalone clarification should be removed, got {missing:?}"
    );
}

#[test]
fn cli_and_daemon_report_version_from_bare_nota_atom() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("version.sock");
    let database_path = temp.path().join("version.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    let version = run_cli(&socket_path, "Version");
    match version {
        Output::VersionReported(report) => {
            assert_eq!(
                report.payload().payload().payload(),
                env!("CARGO_PKG_VERSION")
            );
        }
        other => panic!("expected VersionReported from bare Version input, got {other:?}"),
    }
}

#[test]
fn cli_subscription_receives_matching_intent_events_without_blocking_daemon() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("subscription.sock");
    let database_path = temp.path().join("subscription.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);
    let subscriber = SubscriberProcess::spawn(
        &socket_path,
        "(SubscribeIntent ((Full [(Kinship Rapport)]) Any Any Any (Some Decision) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );

    match subscriber.next_output(Duration::from_secs(2)) {
        Output::SubscriptionStarted(subscription) => {
            assert_eq!(subscription.payload().payload().payload(), &1);
        }
        other => panic!("expected SubscriptionStarted, got {other:?}"),
    }

    let nonmatching = run_cli(
        &socket_path,
        &record_nota(
            "[(Information Documentation)]",
            "Decision",
            "this should not be pushed",
        ),
    );
    assert!(
        matches!(nonmatching, Output::RecordAccepted(_)),
        "ordinary record request should complete while subscription is open, got {nonmatching:?}"
    );
    subscriber.assert_no_output(Duration::from_millis(200));

    let matching = run_cli(
        &socket_path,
        &record_nota(
            "[(Kinship Rapport)]",
            "Decision",
            "subscriber receives this",
        ),
    );
    let Output::RecordAccepted(receipt) = matching else {
        panic!("expected matching RecordAccepted, got {matching:?}");
    };

    match subscriber.next_output(Duration::from_secs(2)) {
        Output::Event(IntentEvent::IntentRecorded(recorded)) => {
            assert_eq!(
                recorded.entry.domains,
                domain_fixtures::domains(&["relating"])
            );
            assert_eq!(recorded.entry.kind, Kind::Decision);
            assert_eq!(
                recorded.entry.description.payload(),
                "subscriber receives this"
            );
            assert_eq!(&recorded.record_identifier, receipt.payload());
        }
        other => panic!("expected IntentRecorded event, got {other:?}"),
    }
}

#[test]
fn cli_and_daemon_classify_state_into_provisional_record() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("state.sock");
    let database_path = temp.path().join("state.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    let accepted = run_cli(&socket_path, "(State [daemon raw intent])");
    match accepted {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected State to classify into RecordAccepted, got {other:?}"),
    }

    let observed = run_cli(
        &socket_path,
        "(Observe ((Full [(Information Documentation)]) Any Any Any (Some Clarification) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );
    let Output::RecordsStashed(stashed) = observed else {
        panic!("expected classified State observation to be stashed, got {observed:?}");
    };
    assert_eq!(*stashed.record_count.payload(), 1);
    assert_eq!(stashed.observed_records.payload().payload().len(), 1);
    assert_eq!(
        stashed.observed_records.payload().payload()[0]
            .entry
            .domains,
        domain_fixtures::domains(&["meaning"])
    );
    assert_eq!(
        stashed.observed_records.payload().payload()[0].entry.kind,
        Kind::Clarification
    );
    assert_eq!(
        stashed.observed_records.payload().payload()[0]
            .entry
            .description
            .payload(),
        "daemon raw intent"
    );
    assert_eq!(
        stashed.observed_records.payload().payload()[0]
            .entry
            .certainty,
        Magnitude::Minimum
    );
    assert_eq!(
        stashed.observed_records.payload().payload()[0]
            .entry
            .privacy,
        Magnitude::Zero
    );

    let looked_up = run_cli(
        &socket_path,
        &format!("(LookupStash {})", stashed.stash_handle.payload()),
    );
    match looked_up {
        Output::RecordsObserved(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.domains,
                domain_fixtures::domains(&["meaning"])
            );
            assert_eq!(
                records.payload().payload()[0].entry.kind,
                Kind::Clarification
            );
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "daemon raw intent"
            );
            assert_eq!(
                records.payload().payload()[0].entry.certainty,
                Magnitude::Minimum
            );
            assert_eq!(
                records.payload().payload()[0].entry.privacy,
                Magnitude::Zero
            );
        }
        other => panic!("expected LookupStash to return classified State record, got {other:?}"),
    }
}

#[test]
fn cli_and_daemon_change_certainty_without_changing_record_identifier() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("change-certainty.sock");
    let database_path = temp.path().join("change-certainty.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    let accepted = run_cli(
        &socket_path,
        &record_nota(
            "[(Information Documentation)]",
            "Correction",
            "certainty target",
        ),
    );
    let record_identifier = match accepted {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
            receipt.payload().clone()
        }
        other => panic!("expected RecordAccepted before certainty change, got {other:?}"),
    };

    let changed = run_cli(
        &socket_path,
        &format!(
            "(ChangeCertainty ({} Zero))",
            record_identifier_argument(&record_identifier)
        ),
    );
    match changed {
        Output::CertaintyChanged(receipt) => {
            assert_eq!(receipt.record_identifier, record_identifier);
            assert_eq!(receipt.certainty, Magnitude::Zero);
        }
        other => panic!("expected CertaintyChanged, got {other:?}"),
    }

    let hidden_from_default_query = run_cli(
        &socket_path,
        "(Observe ((Full [(Information Documentation)]) Any Any Any (Some Correction) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );
    assert!(
        matches!(hidden_from_default_query, Output::Error(_)),
        "zero-certainty records are hidden from ordinary observation, got {hidden_from_default_query:?}"
    );

    let explicit_candidate_query = run_cli(
        &socket_path,
        "(Observe ((Full [(Information Documentation)]) Any Any Any (Some Correction) (Exact Zero) (ExactCertainty Zero) Any))",
    );
    match explicit_candidate_query {
        Output::RecordsStashed(stashed) => {
            assert_eq!(*stashed.record_count.payload(), 1);
        }
        other => panic!("expected explicit zero-certainty query to stash record, got {other:?}"),
    }

    let found = run_cli(
        &socket_path,
        &format!(
            "(Lookup {})",
            record_identifier_argument(&record_identifier)
        ),
    );
    match found {
        Output::RecordFound(record) => {
            assert_eq!(record.record_identifier, record_identifier);
            assert_eq!(record.entry.description.payload(), "certainty target");
            assert_eq!(record.entry.certainty, Magnitude::Zero);
        }
        other => panic!("expected changed record lookup, got {other:?}"),
    }
}

#[test]
fn cli_and_daemon_change_record_replaces_entry_under_same_identifier() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("change-record.sock");
    let database_path = temp.path().join("change-record.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    let accepted = run_cli(
        &socket_path,
        &record_nota(
            "[(Information Documentation)]",
            "Decision",
            "original record",
        ),
    );
    let record_identifier = match accepted {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
            receipt.payload().clone()
        }
        other => panic!("expected RecordAccepted before record change, got {other:?}"),
    };

    let changed = run_cli(
        &socket_path,
        &format!(
            "(ChangeRecord ({} ([(Information Documentation)] Correction [replacement record] High Minimum Zero [process-boundary-test]) ([([replacement record] None)] [replacement record])))",
            record_identifier_argument(&record_identifier)
        ),
    );
    match changed {
        Output::RecordChanged(receipt) => {
            assert_eq!(receipt.payload().payload(), &record_identifier);
        }
        other => panic!("expected RecordChanged, got {other:?}"),
    }

    let found = run_cli(
        &socket_path,
        &format!(
            "(Lookup {})",
            record_identifier_argument(&record_identifier)
        ),
    );
    match found {
        Output::RecordFound(record) => {
            assert_eq!(record.record_identifier, record_identifier);
            assert_eq!(record.entry.domains, domain_fixtures::domains(&["meaning"]));
            assert_eq!(record.entry.kind, Kind::Correction);
            assert_eq!(record.entry.description.payload(), "replacement record");
            assert_eq!(record.entry.certainty, Magnitude::High);
            assert_eq!(record.entry.privacy, Magnitude::Zero);
        }
        other => panic!("expected changed record lookup, got {other:?}"),
    }

    let missing_old_query = run_cli(
        &socket_path,
        "(Observe ((Full [(Information Documentation)]) Any Any Any (Some Decision) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );
    assert!(
        matches!(missing_old_query, Output::Error(_)),
        "the original entry should be replaced, got {missing_old_query:?}"
    );
}

#[test]
fn cli_renders_alias_payload_outputs_without_wrapper_repetition() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("alias-payload.sock");
    let database_path = temp.path().join("alias-payload.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    let rejected = Command::new(env!("CARGO_BIN_EXE_spirit"))
        .env("SPIRIT_SOCKET", &socket_path)
        .arg(record_nota("[]", "Constraint", "alias payload rejection"))
        .output()
        .expect("run cli");
    assert!(
        rejected.status.success(),
        "cli stderr: {}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    let rejected_stdout = String::from_utf8(rejected.stdout).expect("cli stdout is UTF-8");
    assert_eq!(
        rejected_stdout.trim(),
        "(Rejected EmptyDomain)",
        "Rejected aliases must render the direct SignalRejection payload without a Rejected wrapper"
    );
    let rejected_output = Output::from_str(rejected_stdout.trim()).unwrap_or_else(|error| {
        panic!("schema-emitted Output::FromStr on rejection stdout: {error}")
    });
    assert!(
        matches!(rejected_output, Output::Rejected(_)),
        "parsed rejection should be direct Output::Rejected payload"
    );

    let recorded = Command::new(env!("CARGO_BIN_EXE_spirit"))
        .env("SPIRIT_SOCKET", &socket_path)
        .arg(record_nota(
            "[(Information Documentation)]",
            "Constraint",
            "direct accepted payload",
        ))
        .output()
        .expect("run cli");
    assert!(
        recorded.status.success(),
        "cli stderr: {}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    let recorded_stdout = String::from_utf8(recorded.stdout).expect("cli stdout is UTF-8");
    let recorded_output = Output::from_str(recorded_stdout.trim())
        .unwrap_or_else(|error| panic!("schema-emitted Output::FromStr on record stdout: {error}"));
    match recorded_output {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("parsed record reply should be direct RecordAccepted payload: {other:?}"),
    }
}

#[test]
fn daemon_persists_sema_file_across_a_restart() {
    // The strongest durability proof: a daemon writes the `.sema` file,
    // the daemon process is killed, a NEW daemon process opens the SAME
    // `.sema` file, and the previously recorded entry is still observable
    // and the commit sequence resumes. This is the bead `primary-q2au`
    // claim proven at the real process boundary.
    let temp = TempDir::new().expect("tempdir");
    let database_path = temp.path().join("durable.sema");

    // First daemon: record one entry, then drop (kill) it.
    {
        let socket_path = temp.path().join("first.sock");
        let _daemon = DaemonProcess::spawn(&socket_path, &database_path);
        let recorded = run_cli(
            &socket_path,
            &record_nota(
                "[(Technology (Software (Operations Deployment)))]",
                "Decision",
                "survives restart",
            ),
        );
        match recorded {
            Output::RecordAccepted(receipt) => {
                assert_short_record_identifier(receipt.payload());
            }
            other => panic!("expected RecordAccepted from first daemon, got {other:?}"),
        }
        // _daemon drops here: process killed, sema-engine file handle released.
    }

    assert!(
        database_path.exists(),
        "the .sema file outlives the first daemon process"
    );

    // Second daemon against the SAME .sema file on a fresh socket.
    let socket_path = temp.path().join("second.sock");
    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    let observed = run_cli(
        &socket_path,
        "(Observe ((Full [(Technology (Software (Operations Deployment)))]) Any Any Any (Some Decision) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );
    // Observe returns records inline and a recovery stash handle. LookupStash
    // verifies the same content survived the daemon restart.
    let stash_handle = match observed {
        Output::RecordsStashed(stashed) => {
            assert_eq!(
                *stashed.record_count.payload(),
                1,
                "the restarted daemon observes one durable record"
            );
            assert_eq!(
                stashed.observed_records.payload().payload()[0]
                    .entry
                    .description
                    .payload(),
                "survives restart",
                "the restarted daemon returns durable content inline"
            );
            stashed.stash_handle.clone()
        }
        other => panic!("expected RecordsStashed after restart, got {other:?}"),
    };
    let looked_up = run_cli(
        &socket_path,
        &format!("(LookupStash {})", stash_handle.payload()),
    );
    match looked_up {
        Output::RecordsObserved(records) => {
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "survives restart",
                "the restarted daemon's stash retrieves the durable content"
            );
        }
        other => panic!("expected RecordsObserved from LookupStash, got {other:?}"),
    }

    // The commit ledger resumed: the next record is sequence 2, proving
    // the durable counter persisted across the restart, not just records.
    let next = run_cli(
        &socket_path,
        &record_nota(
            "[(Technology (Software (Operations Deployment)))]",
            "Decision",
            "second after restart",
        ),
    );
    match next {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected RecordAccepted after restart, got {other:?}"),
    }
}

#[test]
fn candidate_daemon_handover_from_production_copy_preserves_original_sema_database() {
    let temp = TempDir::new().expect("tempdir");
    let production_database_path = temp.path().join("production.sema");
    let candidate_database_path = temp.path().join("candidate-copy.sema");

    {
        let socket_path = temp.path().join("production-seed.sock");
        let _daemon = DaemonProcess::spawn(&socket_path, &production_database_path);
        let recorded = run_cli(
            &socket_path,
            &record_nota(
                "[(Technology (Software (Operations Deployment)))]",
                "Constraint",
                "production entry before copy",
            ),
        );
        match recorded {
            Output::RecordAccepted(receipt) => {
                assert_short_record_identifier(receipt.payload());
            }
            other => panic!("expected production seed record, got {other:?}"),
        }
    }

    fs::copy(&production_database_path, &candidate_database_path)
        .expect("copy production .sema database for candidate handover");

    {
        let socket_path = temp.path().join("candidate.sock");
        let _daemon = DaemonProcess::spawn(&socket_path, &candidate_database_path);
        let observed = run_cli(
            &socket_path,
            "(Observe ((Full [(Technology (Software (Operations Deployment)))]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
        );
        assert_eq!(
            stashed_descriptions(&socket_path, observed),
            vec![String::from("production entry before copy")],
            "candidate starts from the copied production SEMA state"
        );

        let candidate_recorded = run_cli(
            &socket_path,
            &record_nota(
                "[(Technology (Software (Operations Deployment)))]",
                "Constraint",
                "candidate-only entry after copy",
            ),
        );
        match candidate_recorded {
            Output::RecordAccepted(receipt) => {
                assert_short_record_identifier(receipt.payload());
            }
            other => panic!("expected candidate record, got {other:?}"),
        }

        let candidate_observed = run_cli(
            &socket_path,
            "(Observe ((Full [(Technology (Software (Operations Deployment)))]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
        );
        assert_eq!(
            stashed_descriptions(&socket_path, candidate_observed),
            vec![
                String::from("candidate-only entry after copy"),
                String::from("production entry before copy"),
            ],
            "candidate writes land only in the copied database"
        );
    }

    {
        let socket_path = temp.path().join("production-after.sock");
        let _daemon = DaemonProcess::spawn(&socket_path, &production_database_path);
        let observed = run_cli(
            &socket_path,
            "(Observe ((Full [(Technology (Software (Operations Deployment)))]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
        );
        assert_eq!(
            stashed_descriptions(&socket_path, observed),
            vec![String::from("production entry before copy")],
            "candidate writes must not mutate the original production SEMA file"
        );

        let production_next = run_cli(
            &socket_path,
            &record_nota(
                "[(Technology (Software (Operations Deployment)))]",
                "Constraint",
                "production entry after handover",
            ),
        );
        match production_next {
            Output::RecordAccepted(receipt) => {
                assert_short_record_identifier(receipt.payload());
            }
            other => panic!("expected production post-handover record, got {other:?}"),
        }
    }
}

#[cfg(feature = "testing-trace")]
#[test]
fn cli_receives_testing_trace_events_from_daemon_trace_socket() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("spirit.sock");
    let trace_socket_path = temp.path().join("spirit-trace.sock");
    let database_path = temp.path().join("spirit.sema");

    let _daemon = DaemonProcess::spawn_with_trace(&socket_path, &database_path, &trace_socket_path);

    let recorded = run_cli_with_trace(
        &socket_path,
        &trace_socket_path,
        &record_nota(
            "[(Technology (Software (Engineering Architecture)))]",
            "Constraint",
            "trace crosses daemon boundary",
        ),
    );
    assert!(
        matches!(recorded.output, Output::RecordAccepted(_)),
        "record reply should still be the first CLI line, got {:?}",
        recorded.output
    );
    // The CLI binds the trace socket for this request, so startup events
    // are timing-dependent at this process boundary. The invariant here
    // is the exact request activation sequence after any lifecycle prefix.
    // instrumentation_logging.rs proves lifecycle tracing with an
    // in-process sink bound before Engine::start.
    recorded.assert_trace_sequence_after_optional_lifecycle_start(&[
        "SignalAdmitted",
        "SignalTriaged",
        "NexusEntered",
        "NexusDecided",
        "NexusEntered",
        "NexusDecided",
        "SemaWriteApplied",
        "NexusEntered",
        "NexusDecided",
        "SignalReplied",
    ]);

    let observed = run_cli_with_trace(
        &socket_path,
        &trace_socket_path,
        "(Observe ((Full [(Technology (Software (Engineering Architecture)))]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );
    // Observe flows through the recursive Nexus loop with Stash and returns
    // both the recovery handle and the observed records.
    // The trace below shows each continuation step: command SEMA read,
    // command Stash effect, then reply.
    assert!(
        matches!(observed.output, Output::RecordsStashed(_)),
        "observe reply should still be the first CLI line, got {:?}",
        observed.output
    );
    observed.assert_trace_sequence(&[
        "SignalAdmitted",
        "SignalTriaged",
        "NexusEntered",
        "NexusDecided",
        "SemaReadObserved",
        "NexusEntered",
        "NexusDecided",
        "NexusEntered",
        "NexusDecided",
        "SignalReplied",
    ]);
}

/// Observe returns records inline with a recovery Stash handle.
fn stashed_descriptions(_socket_path: &Path, output: Output) -> Vec<String> {
    match output {
        Output::RecordsStashed(stashed) => {
            let mut descriptions: Vec<String> = stashed
                .observed_records
                .payload()
                .payload()
                .iter()
                .map(|record| record.entry.description.payload().clone())
                .collect();
            descriptions.sort();
            descriptions
        }
        other => panic!("expected RecordsStashed, got {other:?}"),
    }
}

fn wait_for_socket(path: &Path) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("socket did not appear at {}", path.display());
}
