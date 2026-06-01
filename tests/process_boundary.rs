use std::{
    fs,
    path::Path,
    process::{Child, Command},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use spirit_next::{CommitSequence, Configuration, Output, RecordIdentifier};
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
        "(Record ([[schema]] Constraint [schema creates the interface] Maximum))",
    );
    match recorded {
        Output::RecordAccepted(receipt) => {
            assert_eq!(receipt.record_identifier, RecordIdentifier(1));
            assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(1));
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    }

    let observed = run_cli(
        &socket_path,
        "(Observe ((Full [[schema]]) (Some Constraint)))",
    );
    assert!(
        matches!(observed, Output::RecordsObserved(_)),
        "the daemon observes the recorded entry, got {observed:?}"
    );

    let removed = run_cli(&socket_path, "(Remove 1)");
    assert!(
        matches!(removed, Output::RecordRemoved(_)),
        "the daemon removes the recorded entry, got {removed:?}"
    );

    let missing_after_remove = run_cli(
        &socket_path,
        "(Observe ((Full [[schema]]) (Some Constraint)))",
    );
    assert!(
        matches!(missing_after_remove, Output::Error(_)),
        "the removed entry is no longer observable, got {missing_after_remove:?}"
    );

    let rejected = run_cli(
        &socket_path,
        "(Record ([] Constraint [schema rejects before SEMA] Maximum))",
    );
    assert!(
        matches!(rejected, Output::Rejected(_)),
        "empty topic is rejected before SEMA, got {rejected:?}"
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
            "(Record ([[durable-topic]] Decision [survives restart] Maximum))",
        );
        match recorded {
            Output::RecordAccepted(receipt) => {
                assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(1));
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
        "(Observe ((Full [[durable-topic]]) (Some Decision)))",
    );
    match observed {
        Output::RecordsObserved(records) => {
            assert_eq!(
                records.record_set.0[0].description.0, "survives restart",
                "the restarted daemon observes the entry the first daemon wrote"
            );
        }
        other => panic!("expected RecordsObserved after restart, got {other:?}"),
    }

    // The commit ledger resumed: the next record is sequence 2, proving
    // the durable counter persisted across the restart, not just records.
    let next = run_cli(
        &socket_path,
        "(Record ([[durable-topic]] Decision [second after restart] Maximum))",
    );
    match next {
        Output::RecordAccepted(receipt) => {
            assert_eq!(
                receipt.database_marker.commit_sequence,
                CommitSequence(2),
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
            "(Record ([[handover]] Constraint [production entry before copy] Maximum))",
        );
        match recorded {
            Output::RecordAccepted(receipt) => {
                assert_eq!(receipt.record_identifier, RecordIdentifier(1));
                assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(1));
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
            "(Observe ((Full [[handover]]) (Some Constraint)))",
        );
        assert_eq!(
            observed_descriptions(observed),
            vec![String::from("production entry before copy")],
            "candidate starts from the copied production SEMA state"
        );

        let candidate_recorded = run_cli(
            &socket_path,
            "(Record ([[handover]] Constraint [candidate-only entry after copy] Maximum))",
        );
        match candidate_recorded {
            Output::RecordAccepted(receipt) => {
                assert_eq!(
                    receipt.database_marker.commit_sequence,
                    CommitSequence(2),
                    "candidate write resumes the copied ledger"
                );
            }
            other => panic!("expected candidate record, got {other:?}"),
        }

        let candidate_observed = run_cli(
            &socket_path,
            "(Observe ((Full [[handover]]) (Some Constraint)))",
        );
        assert_eq!(
            observed_descriptions(candidate_observed),
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
            "(Observe ((Full [[handover]]) (Some Constraint)))",
        );
        assert_eq!(
            observed_descriptions(observed),
            vec![String::from("production entry before copy")],
            "candidate writes must not mutate the original production SEMA file"
        );

        let production_next = run_cli(
            &socket_path,
            "(Record ([[handover]] Constraint [production entry after handover] Maximum))",
        );
        match production_next {
            Output::RecordAccepted(receipt) => {
                assert_eq!(
                    receipt.database_marker.commit_sequence,
                    CommitSequence(2),
                    "original production ledger advances from its own state, not the candidate copy"
                );
            }
            other => panic!("expected production post-handover record, got {other:?}"),
        }
    }
}

fn observed_descriptions(output: Output) -> Vec<String> {
    match output {
        Output::RecordsObserved(records) => records
            .record_set
            .0
            .into_iter()
            .map(|entry| entry.description.0)
            .collect(),
        other => panic!("expected RecordsObserved, got {other:?}"),
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
