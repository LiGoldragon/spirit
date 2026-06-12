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

use nota_next::NotaEncode;
#[cfg(feature = "agent-guardian")]
use signal_agent::{
    Completion, CompletionText, Input as AgentInput, Output as AgentOutput, StopReasonText,
    TokenUsage,
};
#[cfg(feature = "agent-guardian")]
use signal_spirit::{
    ConfigurationPath, SpiritGuardianAgentConfiguration, SpiritGuardianTimeoutMilliseconds,
};
use spirit::Configuration;
#[cfg(feature = "testing-trace")]
use spirit::TraceEvent;
#[cfg(feature = "agent-guardian")]
use spirit::schema::nexus::GuardianVerdict;
use spirit::schema::signal::{Domains, IntentEvent, Kind, Magnitude, Output, RecordIdentifier};
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
        let AgentInput::Call(_) = input else {
            panic!("expected guardian Call input, got {input:?}");
        };
        let output = AgentOutput::completed(Completion {
            text: CompletionText::new(GuardianVerdict::Accept.to_nota()),
            stop_reason: StopReasonText::new("stop"),
            usage: TokenUsage {
                prompt_tokens: None,
                completion_tokens: None,
            },
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
            "(ConfigurationWriteRequest ({} (Some {}) {} None None {}))",
            nota_path(socket_path),
            nota_path(&meta_socket_path),
            nota_path(database_path),
            nota_path(&configuration_path)
        );
        #[cfg(feature = "agent-guardian")]
        let request = format!(
            "(ConfigurationWriteRequest ({} (Some {}) {} None (Some ({} None None 5000 None)) {}))",
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
        "(ConfigurationWriteRequest ({} (Some {}) {} None (Some ({} (Some {}) (Some {}) 120000 None)) {}))",
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
        "(Record (({domains} {kind} [{description}] Maximum Minimum Zero []) ([{description}] None)))"
    )
}

fn remove_nota(identifier: &RecordIdentifier) -> String {
    format!(
        "(Remove ({} ([remove record] None)))",
        record_identifier_argument(identifier)
    )
}

fn collect_removal_candidates_nota(query: &str) -> String {
    format!("(CollectRemovalCandidates ({query} ([collect removal candidates] None)))")
}

fn assert_short_record_identifier(identifier: &RecordIdentifier) {
    assert!(
        (4..=7).contains(&identifier.len()),
        "record identifier should use a four-to-seven-character code: {identifier}"
    );
    assert!(
        identifier
            .chars()
            .all(|character| character.is_ascii_digit() || character.is_ascii_lowercase()),
        "record identifier should be lower-base36: {identifier}"
    );
}

fn record_identifier_argument(identifier: &RecordIdentifier) -> String {
    identifier.to_string()
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
        let actual = events.iter().map(TraceEvent::name).collect::<Vec<_>>();
        assert_eq!(actual, expected, "trace lines: {:#?}", self.trace_lines);
    }

    fn assert_trace_sequence_after_optional_lifecycle_start(&self, expected: &[&str]) {
        let events = self.trace_events();
        let mut actual = events.iter().map(TraceEvent::name).collect::<Vec<_>>();
        let lifecycle_start = ["SemaStarted", "NexusStarted", "SignalStarted"];
        if actual.starts_with(&lifecycle_start) {
            actual.drain(..lifecycle_start.len());
        }
        assert_eq!(actual, expected, "trace lines: {:#?}", self.trace_lines);
    }

    fn trace_events(&self) -> Vec<TraceEvent> {
        self.trace_lines
            .iter()
            .map(|line| {
                let event = TraceEvent::from_str(line).unwrap_or_else(|error| {
                    panic!("trace CLI line should be generated NOTA {line:?}: {error}")
                });
                assert_eq!(
                    event.to_string(),
                    *line,
                    "trace CLI line should be canonical NOTA"
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
            "[(Technology (Software (Operations InfrastructureAsCode)))]",
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
    let record_identifier = match recorded {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
            receipt.payload().clone()
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    };

    let observed = run_cli(
        &socket_path,
        "(Observe ((Full [(Information Documentation)]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );
    // Designer 480: Observe now flows through Stash; the slim wire reply
    // carries a handle, not the full record set.
    assert!(
        matches!(observed, Output::RecordsStashed(_)),
        "the daemon stashes the observed records and returns a slim handle, got {observed:?}"
    );

    let removed = run_cli(&socket_path, &remove_nota(&record_identifier));
    assert!(
        matches!(removed, Output::RecordRemoved(_)),
        "the daemon removes the recorded entry, got {removed:?}"
    );

    let missing_after_remove = run_cli(
        &socket_path,
        "(Observe ((Full [(Information Documentation)]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );
    assert!(
        matches!(missing_after_remove, Output::Error(_)),
        "the removed entry is no longer observable, got {missing_after_remove:?}"
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
                Domains::from_strings(vec![String::from("relating")])
            );
            assert_eq!(recorded.entry.kind, Kind::Decision);
            assert_eq!(recorded.entry.description, "subscriber receives this");
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
    assert_eq!(stashed.record_count, 1);

    let looked_up = run_cli(
        &socket_path,
        &format!("(LookupStash {})", stashed.stash_handle),
    );
    match looked_up {
        Output::RecordsObserved(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.domains,
                Domains::from_strings(vec![String::from("meaning")])
            );
            assert_eq!(
                records.payload().payload()[0].entry.kind,
                Kind::Clarification
            );
            assert_eq!(
                records.payload().payload()[0].entry.description,
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
            assert_eq!(stashed.record_count, 1);
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
            assert_eq!(record.entry.description, "certainty target");
            assert_eq!(record.entry.certainty, Magnitude::Zero);
        }
        other => panic!("expected changed record lookup, got {other:?}"),
    }
}

#[test]
fn cli_collect_removal_candidates_accepts_direct_query_shorthand() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("collect-direct.sock");
    let database_path = temp.path().join("collect-direct.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    let accepted = run_cli(
        &socket_path,
        &record_nota(
            "[(Information Documentation)]",
            "Correction",
            "direct collection target",
        ),
    );
    let record_identifier = match accepted {
        Output::RecordAccepted(receipt) => receipt.payload().clone(),
        other => panic!("expected RecordAccepted before collection, got {other:?}"),
    };

    let changed = run_cli(
        &socket_path,
        &format!(
            "(ChangeCertainty ({} Zero))",
            record_identifier_argument(&record_identifier)
        ),
    );
    assert!(
        matches!(changed, Output::CertaintyChanged(_)),
        "record becomes an exact-zero collection candidate, got {changed:?}"
    );

    let collected = run_cli(
        &socket_path,
        &collect_removal_candidates_nota(
            "((Full [(Information Documentation)]) Any Any Any (Some Correction) (Exact Zero) (ExactCertainty Zero) Any)",
        ),
    );
    match collected {
        Output::RemovalCandidatesCollected(report) => {
            let collection = report.payload();
            assert_eq!(collection.removal_archive_records.payload().len(), 1);
            assert_eq!(
                collection.removed_identifiers.payload()[0].payload(),
                &record_identifier
            );
        }
        other => panic!("expected direct collection shorthand to collect record, got {other:?}"),
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
            "(ChangeRecord ({} ([(Information Documentation)] Correction [replacement record] High Minimum Zero []) ([replacement record] None)))",
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
            assert_eq!(
                record.entry.domains,
                Domains::from_strings(vec![String::from("meaning")])
            );
            assert_eq!(record.entry.kind, Kind::Correction);
            assert_eq!(record.entry.description, "replacement record");
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
                "[(Technology (Software (Operations InfrastructureAsCode)))]",
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
        "(Observe ((Full [(Technology (Software (Operations InfrastructureAsCode)))]) Any Any Any (Some Decision) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );
    // Designer 480: Observe stashes the durable result; the slim reply
    // returns the handle + count. Follow up by LookupStash to verify the
    // full content survived the daemon restart.
    let stash_handle = match observed {
        Output::RecordsStashed(stashed) => {
            assert_eq!(
                stashed.record_count, 1,
                "the restarted daemon observes one durable record"
            );
            stashed.stash_handle.clone()
        }
        other => panic!("expected RecordsStashed after restart, got {other:?}"),
    };
    let looked_up = run_cli(&socket_path, &format!("(LookupStash {})", stash_handle));
    match looked_up {
        Output::RecordsObserved(records) => {
            assert_eq!(
                records.payload().payload()[0].entry.description,
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
            "[(Technology (Software (Operations InfrastructureAsCode)))]",
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
                "[(Technology (Software (Operations InfrastructureAsCode)))]",
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
            "(Observe ((Full [(Technology (Software (Operations InfrastructureAsCode)))]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
        );
        assert_eq!(
            stashed_descriptions(&socket_path, observed),
            vec![String::from("production entry before copy")],
            "candidate starts from the copied production SEMA state"
        );

        let candidate_recorded = run_cli(
            &socket_path,
            &record_nota(
                "[(Technology (Software (Operations InfrastructureAsCode)))]",
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
            "(Observe ((Full [(Technology (Software (Operations InfrastructureAsCode)))]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
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
            "(Observe ((Full [(Technology (Software (Operations InfrastructureAsCode)))]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
        );
        assert_eq!(
            stashed_descriptions(&socket_path, observed),
            vec![String::from("production entry before copy")],
            "candidate writes must not mutate the original production SEMA file"
        );

        let production_next = run_cli(
            &socket_path,
            &record_nota(
                "[(Technology (Software (Operations InfrastructureAsCode)))]",
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
            "[(Technology (Software (Engineering SoftwareArchitecture)))]",
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
        "SemaWriteApplied",
        "NexusEntered",
        "NexusDecided",
        "SignalReplied",
    ]);

    let observed = run_cli_with_trace(
        &socket_path,
        &trace_socket_path,
        "(Observe ((Full [(Technology (Software (Engineering SoftwareArchitecture)))]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))",
    );
    // Designer 480: Observe flows through the recursive Nexus loop with
    // Stash; the slim wire reply carries a handle, not the full record set.
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

/// Designer 480: Observe now returns a slim Stash handle; the helper
/// resolves the handle through LookupStash and reads back the descriptions.
fn stashed_descriptions(socket_path: &Path, output: Output) -> Vec<String> {
    let stash_handle = match output {
        Output::RecordsStashed(stashed) => stashed.stash_handle.clone(),
        other => panic!("expected RecordsStashed, got {other:?}"),
    };
    let resolved = run_cli(socket_path, &format!("(LookupStash {})", stash_handle));
    match resolved {
        Output::RecordsObserved(records) => {
            let mut descriptions: Vec<String> = records
                .payload()
                .payload()
                .iter()
                .map(|record| record.entry.description.payload().clone())
                .collect();
            descriptions.sort();
            descriptions
        }
        other => panic!("expected RecordsObserved from LookupStash, got {other:?}"),
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
