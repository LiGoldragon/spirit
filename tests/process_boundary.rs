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

#[cfg(feature = "testing-trace")]
use spirit::TraceEvent;
use spirit::{
    Configuration,
    schema::signal::{IntentEvent, Kind, Magnitude, Output, RecordIdentifier},
};
use tempfile::TempDir;

struct DaemonProcess {
    child: Child,
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

impl Drop for SubscriberProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
    }
}

impl DaemonProcess {
    fn meta_socket_path(socket_path: &Path) -> std::path::PathBuf {
        socket_path.with_extension("meta.sock")
    }

    fn spawn(socket_path: &Path, database_path: &Path) -> Self {
        let configuration_path = socket_path.with_extension("config.rkyv");
        let meta_socket_path = Self::meta_socket_path(socket_path);
        Configuration::new(socket_path, database_path)
            .with_meta_socket_path(&meta_socket_path)
            .write_binary_file(&configuration_path)
            .expect("write binary daemon configuration");
        let child = Command::new(env!("CARGO_BIN_EXE_spirit-daemon"))
            .arg(configuration_path)
            .spawn()
            .expect("spawn daemon");
        let process = Self { child };
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
        Configuration::new_with_trace(socket_path, database_path, trace_socket_path)
            .with_meta_socket_path(&meta_socket_path)
            .write_binary_file(&configuration_path)
            .expect("write binary daemon configuration with trace socket");
        let child = Command::new(env!("CARGO_BIN_EXE_spirit-daemon"))
            .arg(configuration_path)
            .spawn()
            .expect("spawn daemon");
        let process = Self { child };
        wait_for_socket(socket_path);
        wait_for_socket(&meta_socket_path);
        process
    }
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
    format!("[{identifier}]")
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
        "(Record ([schema] Constraint [schema creates the interface] Maximum Zero))",
    );
    let record_identifier = match recorded {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(&receipt.record_identifier);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
            receipt.record_identifier
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    };

    let observed = run_cli(
        &socket_path,
        "(Observe ((Full [schema]) (Some Constraint) (Exact Zero)))",
    );
    // Designer 480: Observe now flows through Stash; the slim wire reply
    // carries a handle, not the full record set.
    assert!(
        matches!(observed, Output::RecordsStashed(_)),
        "the daemon stashes the observed records and returns a slim handle, got {observed:?}"
    );

    let removed = run_cli(
        &socket_path,
        &format!(
            "(Remove {})",
            record_identifier_argument(&record_identifier)
        ),
    );
    assert!(
        matches!(removed, Output::RecordRemoved(_)),
        "the daemon removes the recorded entry, got {removed:?}"
    );

    let missing_after_remove = run_cli(
        &socket_path,
        "(Observe ((Full [schema]) (Some Constraint) (Exact Zero)))",
    );
    assert!(
        matches!(missing_after_remove, Output::Error(_)),
        "the removed entry is no longer observable, got {missing_after_remove:?}"
    );

    let rejected = run_cli(
        &socket_path,
        "(Record ([] Constraint [schema rejects before SEMA] Maximum Zero))",
    );
    assert!(
        matches!(rejected, Output::Rejected(_)),
        "empty topic is rejected before SEMA, got {rejected:?}"
    );
}

#[test]
fn cli_subscription_receives_matching_intent_events_without_blocking_daemon() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("subscription.sock");
    let database_path = temp.path().join("subscription.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);
    let subscriber = SubscriberProcess::spawn(
        &socket_path,
        "(SubscribeIntent ((Full [streaming]) (Some Decision) (Exact Zero)))",
    );

    match subscriber.next_output(Duration::from_secs(2)) {
        Output::SubscriptionStarted(subscription) => {
            assert_eq!(subscription.subscription_token, 1);
            assert_eq!(subscription.database_marker.commit_sequence, 0);
        }
        other => panic!("expected SubscriptionStarted, got {other:?}"),
    }

    let nonmatching = run_cli(
        &socket_path,
        "(Record ([[other]] Decision [this should not be pushed] Maximum Zero))",
    );
    assert!(
        matches!(nonmatching, Output::RecordAccepted(_)),
        "ordinary record request should complete while subscription is open, got {nonmatching:?}"
    );
    subscriber.assert_no_output(Duration::from_millis(200));

    let matching = run_cli(
        &socket_path,
        "(Record ([streaming] Decision [subscriber receives this] Maximum Zero))",
    );
    let Output::RecordAccepted(receipt) = matching else {
        panic!("expected matching RecordAccepted, got {matching:?}");
    };

    match subscriber.next_output(Duration::from_secs(2)) {
        Output::Event(IntentEvent::IntentRecorded(recorded)) => {
            assert_eq!(recorded.entry.topics, vec![String::from("streaming")]);
            assert_eq!(recorded.entry.kind, Kind::Decision);
            assert_eq!(recorded.entry.description, "subscriber receives this");
            assert_eq!(recorded.sema_receipt, receipt);
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

    let accepted = run_cli(&socket_path, "(State ([daemon raw intent]))");
    match accepted {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(&receipt.record_identifier);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
        }
        other => panic!("expected State to classify into RecordAccepted, got {other:?}"),
    }

    let observed = run_cli(
        &socket_path,
        "(Observe ((Full [unclassified]) (Some Clarification) (Exact Zero)))",
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
            assert_eq!(records.record_set.len(), 1);
            assert_eq!(
                records.record_set[0].topics,
                vec![String::from("unclassified")]
            );
            assert_eq!(records.record_set[0].kind, Kind::Clarification);
            assert_eq!(records.record_set[0].description, "daemon raw intent");
            assert_eq!(records.record_set[0].magnitude, Magnitude::Minimum);
            assert_eq!(records.record_set[0].privacy, Magnitude::Zero);
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
        "(Record ([schema] Correction [certainty target] Maximum Zero))",
    );
    let record_identifier = match accepted {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(&receipt.record_identifier);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
            receipt.record_identifier
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
            assert_eq!(receipt.database_marker.commit_sequence, 2);
        }
        other => panic!("expected CertaintyChanged, got {other:?}"),
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
            assert_eq!(record.entry.magnitude, Magnitude::Zero);
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
        "(Record ([schema] Decision [original record] Maximum Zero))",
    );
    let record_identifier = match accepted {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(&receipt.record_identifier);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
            receipt.record_identifier
        }
        other => panic!("expected RecordAccepted before record change, got {other:?}"),
    };

    let changed = run_cli(
        &socket_path,
        &format!(
            "(ChangeRecord ({} ([[schema mutation]] Correction [replacement record] High Zero)))",
            record_identifier_argument(&record_identifier)
        ),
    );
    match changed {
        Output::RecordChanged(receipt) => {
            assert_eq!(receipt.record_identifier, record_identifier);
            assert_eq!(receipt.database_marker.commit_sequence, 2);
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
            assert_eq!(record.entry.topics, vec![String::from("schema mutation")]);
            assert_eq!(record.entry.kind, Kind::Correction);
            assert_eq!(record.entry.description, "replacement record");
            assert_eq!(record.entry.magnitude, Magnitude::High);
            assert_eq!(record.entry.privacy, Magnitude::Zero);
        }
        other => panic!("expected changed record lookup, got {other:?}"),
    }

    let missing_old_query = run_cli(
        &socket_path,
        "(Observe ((Full [schema]) (Some Decision) (Exact Zero)))",
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
        .arg("(Record ([] Constraint [alias payload rejection] Maximum Zero))")
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
        "(Rejected (EmptyTopic (0 0)))",
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
        .arg("(Record ([[alias-payload]] Constraint [direct accepted payload] Maximum Zero))")
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
            assert_short_record_identifier(&receipt.record_identifier);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
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
            "(Record ([durable-topic] Decision [survives restart] Maximum Zero))",
        );
        match recorded {
            Output::RecordAccepted(receipt) => {
                assert_eq!(receipt.database_marker.commit_sequence, 1);
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
        "(Observe ((Full [durable-topic]) (Some Decision) (Exact Zero)))",
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
            stashed.stash_handle
        }
        other => panic!("expected RecordsStashed after restart, got {other:?}"),
    };
    let looked_up = run_cli(&socket_path, &format!("(LookupStash {})", stash_handle));
    match looked_up {
        Output::RecordsObserved(records) => {
            assert_eq!(
                records.record_set[0].description, "survives restart",
                "the restarted daemon's stash retrieves the durable content"
            );
        }
        other => panic!("expected RecordsObserved from LookupStash, got {other:?}"),
    }

    // The commit ledger resumed: the next record is sequence 2, proving
    // the durable counter persisted across the restart, not just records.
    let next = run_cli(
        &socket_path,
        "(Record ([durable-topic] Decision [second after restart] Maximum Zero))",
    );
    match next {
        Output::RecordAccepted(receipt) => {
            assert_eq!(
                receipt.database_marker.commit_sequence, 2,
                "commit sequence resumes from the persisted ledger after restart"
            );
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
            "(Record ([handover] Constraint [production entry before copy] Maximum Zero))",
        );
        match recorded {
            Output::RecordAccepted(receipt) => {
                assert_short_record_identifier(&receipt.record_identifier);
                assert_eq!(receipt.database_marker.commit_sequence, 1);
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
            "(Observe ((Full [handover]) (Some Constraint) (Exact Zero)))",
        );
        assert_eq!(
            stashed_descriptions(&socket_path, observed),
            vec![String::from("production entry before copy")],
            "candidate starts from the copied production SEMA state"
        );

        let candidate_recorded = run_cli(
            &socket_path,
            "(Record ([handover] Constraint [candidate-only entry after copy] Maximum Zero))",
        );
        match candidate_recorded {
            Output::RecordAccepted(receipt) => {
                assert_eq!(
                    receipt.database_marker.commit_sequence, 2,
                    "candidate write resumes the copied ledger"
                );
            }
            other => panic!("expected candidate record, got {other:?}"),
        }

        let candidate_observed = run_cli(
            &socket_path,
            "(Observe ((Full [handover]) (Some Constraint) (Exact Zero)))",
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
            "(Observe ((Full [handover]) (Some Constraint) (Exact Zero)))",
        );
        assert_eq!(
            stashed_descriptions(&socket_path, observed),
            vec![String::from("production entry before copy")],
            "candidate writes must not mutate the original production SEMA file"
        );

        let production_next = run_cli(
            &socket_path,
            "(Record ([handover] Constraint [production entry after handover] Maximum Zero))",
        );
        match production_next {
            Output::RecordAccepted(receipt) => {
                assert_eq!(
                    receipt.database_marker.commit_sequence, 2,
                    "original production ledger advances from its own state, not the candidate copy"
                );
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
        "(Record ([trace] Constraint [trace crosses daemon boundary] Maximum Zero))",
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
        "SemaWriteApplied",
        "NexusDecided",
        "SignalReplied",
    ]);

    let observed = run_cli_with_trace(
        &socket_path,
        &trace_socket_path,
        "(Observe ((Full [trace]) (Some Constraint) (Exact Zero)))",
    );
    // Designer 480: Observe flows through the recursive Nexus loop with
    // Stash; the slim wire reply carries a handle, not the full record set.
    // The trace below shows the loop ran SEMA once and Nexus once — the
    // additional Effect step is internal to one NexusEngine::execute call.
    assert!(
        matches!(observed.output, Output::RecordsStashed(_)),
        "observe reply should still be the first CLI line, got {:?}",
        observed.output
    );
    observed.assert_trace_sequence(&[
        "SignalAdmitted",
        "SignalTriaged",
        "NexusEntered",
        "SemaReadObserved",
        "NexusDecided",
        "SignalReplied",
    ]);
}

/// Designer 480: Observe now returns a slim Stash handle; the helper
/// resolves the handle through LookupStash and reads back the descriptions.
fn stashed_descriptions(socket_path: &Path, output: Output) -> Vec<String> {
    let stash_handle = match output {
        Output::RecordsStashed(stashed) => stashed.stash_handle,
        other => panic!("expected RecordsStashed, got {other:?}"),
    };
    let resolved = run_cli(socket_path, &format!("(LookupStash {})", stash_handle));
    match resolved {
        Output::RecordsObserved(records) => {
            let mut descriptions: Vec<String> = records
                .record_set
                .into_iter()
                .map(|entry| entry.description)
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
