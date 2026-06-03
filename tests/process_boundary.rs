use std::{
    fs,
    path::Path,
    process::{Child, Command},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "testing-trace")]
use spirit_next::TraceEvent;
use spirit_next::{Configuration, Output};
use tempfile::TempDir;

struct DaemonProcess {
    child: Child,
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl DaemonProcess {
    fn spawn(socket_path: &Path, database_path: &Path) -> Self {
        let configuration_path = socket_path.with_extension("config.rkyv");
        Configuration::new(socket_path, database_path)
            .write_binary_file(&configuration_path)
            .expect("write binary daemon configuration");
        let child = Command::new(env!("CARGO_BIN_EXE_spirit-next-daemon"))
            .arg(configuration_path)
            .spawn()
            .expect("spawn daemon");
        let process = Self { child };
        wait_for_socket(socket_path);
        process
    }

    #[cfg(feature = "testing-trace")]
    fn spawn_with_trace(
        socket_path: &Path,
        database_path: &Path,
        trace_socket_path: &Path,
    ) -> Self {
        let configuration_path = socket_path.with_extension("config.rkyv");
        Configuration::new_with_trace(socket_path, database_path, trace_socket_path)
            .write_binary_file(&configuration_path)
            .expect("write binary daemon configuration with trace socket");
        let child = Command::new(env!("CARGO_BIN_EXE_spirit-next-daemon"))
            .arg(configuration_path)
            .spawn()
            .expect("spawn daemon");
        let process = Self { child };
        wait_for_socket(socket_path);
        process
    }
}

fn run_cli(socket_path: &Path, nota_argument: &str) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_spirit-next"))
        .env("SPIRIT_NEXT_SOCKET", socket_path)
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
    let output = Command::new(env!("CARGO_BIN_EXE_spirit-next"))
        .env("SPIRIT_NEXT_SOCKET", socket_path)
        .env("SPIRIT_NEXT_TRACE_SOCKET", trace_socket_path)
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
    let socket_path = temp.path().join("spirit-next.sock");
    let database_path = temp.path().join("spirit-next.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    // Record path — parsed back into the schema-emitted Output, asserted
    // on the typed variant (not on the raw digest string, which is now a
    // real content hash).
    let recorded = run_cli(
        &socket_path,
        "(Record ([[schema]] Constraint [schema creates the interface] Maximum Zero))",
    );
    match recorded {
        Output::RecordAccepted(receipt) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    }

    let observed = run_cli(
        &socket_path,
        "(Observe ((Full [[schema]]) (Some Constraint) (Exact Zero)))",
    );
    // Designer 480: Observe now flows through Stash; the slim wire reply
    // carries a handle, not the full record set.
    assert!(
        matches!(observed, Output::RecordsStashed(_)),
        "the daemon stashes the observed records and returns a slim handle, got {observed:?}"
    );

    let removed = run_cli(&socket_path, "(Remove 1)");
    assert!(
        matches!(removed, Output::RecordRemoved(_)),
        "the daemon removes the recorded entry, got {removed:?}"
    );

    let missing_after_remove = run_cli(
        &socket_path,
        "(Observe ((Full [[schema]]) (Some Constraint) (Exact Zero)))",
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
fn cli_renders_alias_payload_outputs_without_wrapper_repetition() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("alias-payload.sock");
    let database_path = temp.path().join("alias-payload.sema");

    let _daemon = DaemonProcess::spawn(&socket_path, &database_path);

    let rejected = Command::new(env!("CARGO_BIN_EXE_spirit-next"))
        .env("SPIRIT_NEXT_SOCKET", &socket_path)
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

    let recorded = Command::new(env!("CARGO_BIN_EXE_spirit-next"))
        .env("SPIRIT_NEXT_SOCKET", &socket_path)
        .arg("(Record ([[alias-payload]] Constraint [direct accepted payload] Maximum Zero))")
        .output()
        .expect("run cli");
    assert!(
        recorded.status.success(),
        "cli stderr: {}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    let recorded_stdout = String::from_utf8(recorded.stdout).expect("cli stdout is UTF-8");
    assert!(
        recorded_stdout.trim().starts_with("(RecordAccepted (1 (1 "),
        "RecordAccepted aliases must render the direct SemaReceipt payload, got {recorded_stdout:?}"
    );
    let recorded_output = Output::from_str(recorded_stdout.trim())
        .unwrap_or_else(|error| panic!("schema-emitted Output::FromStr on record stdout: {error}"));
    assert!(
        matches!(recorded_output, Output::RecordAccepted(_)),
        "parsed record reply should be direct Output::RecordAccepted payload"
    );
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
            "(Record ([[durable-topic]] Decision [survives restart] Maximum Zero))",
        );
        match recorded {
            Output::RecordAccepted(receipt) => {
                assert_eq!(receipt.database_marker.commit_sequence, 1);
            }
            other => panic!("expected RecordAccepted from first daemon, got {other:?}"),
        }
        // _daemon drops here: process killed, redb file handle released.
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
        "(Observe ((Full [[durable-topic]]) (Some Decision) (Exact Zero)))",
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
        "(Record ([[durable-topic]] Decision [second after restart] Maximum Zero))",
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
            "(Record ([[handover]] Constraint [production entry before copy] Maximum Zero))",
        );
        match recorded {
            Output::RecordAccepted(receipt) => {
                assert_eq!(receipt.record_identifier, 1);
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
            "(Observe ((Full [[handover]]) (Some Constraint) (Exact Zero)))",
        );
        assert_eq!(
            stashed_descriptions(&socket_path, observed),
            vec![String::from("production entry before copy")],
            "candidate starts from the copied production SEMA state"
        );

        let candidate_recorded = run_cli(
            &socket_path,
            "(Record ([[handover]] Constraint [candidate-only entry after copy] Maximum Zero))",
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
            "(Observe ((Full [[handover]]) (Some Constraint) (Exact Zero)))",
        );
        assert_eq!(
            stashed_descriptions(&socket_path, candidate_observed),
            vec![
                String::from("production entry before copy"),
                String::from("candidate-only entry after copy"),
            ],
            "candidate writes land only in the copied database"
        );
    }

    {
        let socket_path = temp.path().join("production-after.sock");
        let _daemon = DaemonProcess::spawn(&socket_path, &production_database_path);
        let observed = run_cli(
            &socket_path,
            "(Observe ((Full [[handover]]) (Some Constraint) (Exact Zero)))",
        );
        assert_eq!(
            stashed_descriptions(&socket_path, observed),
            vec![String::from("production entry before copy")],
            "candidate writes must not mutate the original production SEMA file"
        );

        let production_next = run_cli(
            &socket_path,
            "(Record ([[handover]] Constraint [production entry after handover] Maximum Zero))",
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
    let socket_path = temp.path().join("spirit-next.sock");
    let trace_socket_path = temp.path().join("spirit-next-trace.sock");
    let database_path = temp.path().join("spirit-next.sema");

    let _daemon = DaemonProcess::spawn_with_trace(&socket_path, &database_path, &trace_socket_path);

    let recorded = run_cli_with_trace(
        &socket_path,
        &trace_socket_path,
        "(Record ([[trace]] Constraint [trace crosses daemon boundary] Maximum Zero))",
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
        "(Observe ((Full [[trace]]) (Some Constraint) (Exact Zero)))",
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
        Output::RecordsObserved(records) => records
            .record_set
            .into_iter()
            .map(|entry| entry.description)
            .collect(),
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
