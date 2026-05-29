//! Nix-driven integration tests for the spirit-next schema-driven stack.
//!
//! These tests are the cycle-3-prep forcing function for record 1006
//! (Maximum, 2026-05-27): tests must PROVE not pretend; the canonical
//! shape is schema files driving Nix-built binaries that the test
//! launches and exchanges real rkyv signal frames with over a real
//! Unix socket. Every component — `spirit-next` (CLI), `spirit-next-
//! daemon` (daemon, holds the SignalActor + Engine + Store triad) — is
//! the SAME schema-emitted code path the runtime uses.
//!
//! ## Discipline
//!
//! Where the in-process tests in `runtime_triad.rs` exercise
//! `Engine::handle` directly, these tests run the actual binaries the
//! Nix package built and assert on the CLI's stdout — the same surface
//! a human would see. Each test:
//!
//!   1. Locates the Nix-built binaries (via `nix build` invoked by the
//!      test, or via `SPIRIT_NEXT_NIX_BUILD_RESULT` if pre-built).
//!   2. Spawns the daemon binary against a temp Unix socket.
//!   3. Invokes the CLI binary against that socket, passing inline
//!      NOTA arguments — the SAME single-NOTA-argument contract
//!      `AGENTS.md` mandates and the schema-driven runtime parses.
//!   4. Asserts on the CLI's stdout (which IS the schema-emitted
//!      `Output::to_string()` NOTA round-trip) by parsing it back into
//!      the schema-emitted `Output` enum and matching typed variants —
//!      never against raw strings (record 995/996/997).
//!
//! ## Why these tests are `#[ignore]`
//!
//! Invoking `nix build` from inside `cargo test` is heavy (8s+ on a
//! warm remote builder, longer on a cold build). They run via:
//!
//! ```bash
//! cargo test --test nix_integration -- --ignored
//! ```
//!
//! or through the flake app:
//!
//! ```bash
//! nix run .#nix-integration-tests
//! ```
//!
//! The script seeds `SPIRIT_NEXT_NIX_BUILD_RESULT` with a pre-built
//! binary directory so the test bypasses its own `nix build`.
//!
//! ## What each test proves
//!
//! - `nix_built_spirit_cli_records_through_real_socket_to_nix_built_daemon`
//!   — happy-path Record traverses CLI binary → Unix socket → daemon
//!   binary → SignalActor → Engine → Store, returning `RecordAccepted`.
//!   The test parses the CLI's stdout back through the schema-emitted
//!   `Output::FromStr` and matches typed variants.
//!
//! - `nix_built_daemon_rejects_invalid_input_through_schema_emitted_rejection`
//!   — invalid Input is rejected by `SignalActor::accept` and the
//!   schema-emitted `SignalRejection` flows back to the CLI through
//!   the rkyv signal frame.
//!
//! - `nix_built_daemon_persists_state_across_two_cli_invocations`
//!   — two CLI invocations against the same daemon process show
//!   monotonic `CommitSequence` advancement on the schema-emitted
//!   `DatabaseMarker`.
//!
//! - `nix_built_daemon_observes_recorded_entries_back_through_query`
//!   — a Record followed by an Observe Query returns the schema-emitted
//!   `RecordsObserved` variant with the entry payload echoed.
//!
//! - `nix_built_daemon_returns_missed_when_no_matching_record_exists`
//!   — Observe against an empty store returns the schema-emitted
//!   `Output::Error(ErrorReport)` (the SEMA-plane "no matching record"
//!   path).
//!
//! - `nix_built_daemon_handles_back_to_back_inputs_through_one_socket`
//!   — the same daemon process serves multiple sequential CLI
//!   invocations without restart.
//!
//! - `nix_build_default_package_emits_both_binaries`
//!   — sanity-check the Nix build produces both `bin/spirit-next` and
//!   `bin/spirit-next-daemon`, proving the schema-driven build pipeline
//!   reaches the binary stage.
//!
//! - `nix_built_binaries_carry_schema_emitted_round_trip_for_every_output_variant`
//!   — for each schema-emitted `Output` variant (`RecordAccepted`,
//!   `RecordsObserved`, `Error`, `Rejected`), drive the CLI to produce
//!   that variant and parse the stdout back through the schema-emitted
//!   `Output::FromStr` — proving the NOTA wire form on the wire and
//!   the FromStr surface stay in sync.

use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    str::FromStr,
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use spirit_next::{
    CommitSequence, DatabaseMarker, Description, Entry, ErrorReport, Kind, Magnitude, Output,
    OutputRoute, RecordIdentifier, SemaReceipt, SignalRejection, StateDigest, Topic, Topics,
    ValidationError,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Nix-build harness: locate the schema-driven binaries.
// ---------------------------------------------------------------------------

/// Outcome of locating Nix-built spirit-next binaries.
///
/// Tests call `NixBuiltBinaries::ensure()` which either reuses
/// `SPIRIT_NEXT_NIX_BUILD_RESULT` (a pre-built `result/` directory
/// containing `bin/spirit-next` + `bin/spirit-next-daemon`) or invokes
/// `nix build` against the workspace flake — the SAME build the
/// schema-driven check derivation runs.
#[derive(Debug, Clone)]
struct NixBuiltBinaries {
    spirit_cli: PathBuf,
    spirit_daemon: PathBuf,
}

impl NixBuiltBinaries {
    fn ensure() -> Self {
        nix_built_binaries().clone()
    }
}

/// One-shot global so multiple tests share a single `nix build` invocation.
fn nix_built_binaries() -> &'static NixBuiltBinaries {
    static BINARIES: OnceLock<NixBuiltBinaries> = OnceLock::new();
    BINARIES.get_or_init(NixBuiltBinaries::locate_or_build)
}

impl NixBuiltBinaries {
    fn locate_or_build() -> Self {
        if let Ok(directory) = env::var("SPIRIT_NEXT_NIX_BUILD_RESULT") {
            return Self::from_directory(&PathBuf::from(directory));
        }
        Self::from_directory(&Self::nix_build())
    }

    fn from_directory(directory: &Path) -> Self {
        let spirit_cli = directory.join("bin").join("spirit-next");
        let spirit_daemon = directory.join("bin").join("spirit-next-daemon");
        assert!(
            spirit_cli.exists(),
            "expected Nix-built CLI binary at {}",
            spirit_cli.display()
        );
        assert!(
            spirit_daemon.exists(),
            "expected Nix-built daemon binary at {}",
            spirit_daemon.display()
        );
        Self {
            spirit_cli,
            spirit_daemon,
        }
    }

    /// Run `nix build` against the workspace flake — the same path
    /// `scripts/check-local-schema-stack` uses, with the same local
    /// schema-stack overrides so the build picks up the workspace's
    /// upstream checkouts of `nota-next`/`schema-next`/`schema-rust-next`
    /// rather than the pinned remote revisions.
    fn nix_build() -> PathBuf {
        let repo_root = repo_root();
        let temp_link = repo_root.join("target").join("nix-integration-result");
        if temp_link.exists() {
            let _ = std::fs::remove_file(&temp_link);
        }

        let mut command = Command::new("nix");
        command
            .arg("build")
            .arg("--print-out-paths")
            .arg("--no-link");
        for (input, path) in nix_input_overrides() {
            command.arg("--override-input").arg(input).arg(path);
        }
        command.arg(format!("path:{}#default", repo_root.display()));

        let output = command.output().expect("invoke nix build");
        assert!(
            output.status.success(),
            "nix build failed (status {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let store_path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        assert!(
            !store_path.is_empty(),
            "nix build emitted no store path; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        PathBuf::from(store_path)
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn nix_input_overrides() -> Vec<(&'static str, String)> {
    let workspace_default = |name: &str, env_key: &str| -> String {
        env::var(env_key).unwrap_or_else(|_| format!("/git/github.com/LiGoldragon/{name}"))
    };
    vec![
        (
            "nota-next-source",
            workspace_default("nota-next", "NOTA_NEXT_PATH"),
        ),
        (
            "schema-next-source",
            workspace_default("schema-next", "SCHEMA_NEXT_PATH"),
        ),
        (
            "schema-rust-next-source",
            workspace_default("schema-rust-next", "SCHEMA_RUST_NEXT_PATH"),
        ),
    ]
    .into_iter()
    .map(|(input, path)| (input, format!("path:{path}")))
    .collect()
}

// ---------------------------------------------------------------------------
// Daemon launch + CLI exchange — process boundary helpers.
// ---------------------------------------------------------------------------

/// RAII handle to a spawned daemon process. Drop kills the process.
struct DaemonProcess {
    child: Child,
    socket_path: PathBuf,
    #[allow(dead_code)]
    temp_directory: TempDir,
}

impl DaemonProcess {
    fn spawn(binaries: &NixBuiltBinaries) -> Self {
        let temp_directory = TempDir::new().expect("create tempdir");
        let socket_path = temp_directory.path().join("spirit-next.sock");
        let socket_argument = format!("[{}]", socket_path.display());

        let child = Command::new(&binaries.spirit_daemon)
            .arg(socket_argument)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn daemon");

        let process = Self {
            child,
            socket_path,
            temp_directory,
        };
        process.wait_for_socket();
        process
    }

    fn wait_for_socket(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.socket_path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "daemon socket did not appear at {}",
            self.socket_path.display()
        );
    }

    fn socket(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Run the CLI binary against the daemon's socket with one NOTA
/// argument. Returns the parsed schema-emitted `Output` — no raw
/// string assertions in callers (record 995/996/997).
fn run_cli_for_output(binaries: &NixBuiltBinaries, socket: &Path, nota_argument: &str) -> Output {
    let output = Command::new(&binaries.spirit_cli)
        .arg(nota_argument)
        .env("SPIRIT_NEXT_SOCKET", socket)
        .output()
        .expect("run CLI");
    assert!(
        output.status.success(),
        "spirit-next CLI failed (status {}): stderr={}; stdout={}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8(output.stdout).expect("CLI stdout is UTF-8");
    let trimmed = stdout.trim_end();
    Output::from_str(trimmed).unwrap_or_else(|error| {
        panic!("schema-emitted Output::FromStr on CLI stdout {trimmed:?}: {error}")
    })
}

// ---------------------------------------------------------------------------
// Schema-emitted fixture constructors — never strings as state.
// ---------------------------------------------------------------------------

fn entry(description: &str) -> Entry {
    Entry {
        topics: Topics(vec![Topic(String::from("nix-integration"))]),
        kind: Kind::Decision,
        description: Description(String::from(description)),
        magnitude: Magnitude::Maximum,
    }
}

fn marker(commit_sequence: u64, state_digest: u64) -> DatabaseMarker {
    DatabaseMarker {
        commit_sequence: CommitSequence(commit_sequence),
        state_digest: StateDigest(state_digest),
    }
}

// ---------------------------------------------------------------------------
// Tests — every one of these proves the schema-driven build pipeline
// AND the runtime end-to-end, through the same binaries Nix builds.
// ---------------------------------------------------------------------------

/// Sanity check: the Nix build actually produces both binaries from
/// the schema-driven build pipeline. If THIS fails, every other test
/// below is meaningless — so it runs first and fast.
#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_build_default_package_emits_both_binaries() {
    let binaries = NixBuiltBinaries::ensure();

    // PROOF: both binaries exist as executables on the Nix store path.
    let metadata_cli = std::fs::metadata(&binaries.spirit_cli).expect("stat CLI");
    let metadata_daemon = std::fs::metadata(&binaries.spirit_daemon).expect("stat daemon");
    assert!(metadata_cli.is_file());
    assert!(metadata_daemon.is_file());
    assert!(
        is_executable(&metadata_cli),
        "CLI binary at {} must be executable",
        binaries.spirit_cli.display()
    );
    assert!(
        is_executable(&metadata_daemon),
        "daemon binary at {} must be executable",
        binaries.spirit_daemon.display()
    );
}

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_spirit_cli_records_through_real_socket_to_nix_built_daemon() {
    // PATTERN: Record path end-to-end through the actual binaries Nix
    // built. The CLI binary opens a Unix socket to the daemon binary,
    // sends an rkyv-encoded signal frame, the daemon's SignalActor +
    // Engine + Store triad runs, the schema-emitted SemaReceipt comes
    // back through the reverse plane chain, the CLI writes the NOTA
    // round-trip to stdout, and we parse it back into the schema-
    // emitted Output enum.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    let nota_input =
        "(Record ([[nix-integration]] Decision [end to end through nix built binaries] Maximum))";
    let output = run_cli_for_output(&binaries, daemon.socket(), nota_input);

    // SCHEMA-TYPED ASSERTION: not on the string, on the parsed schema-emitted variant.
    match output {
        Output::RecordAccepted(SemaReceipt {
            record_identifier,
            database_marker,
        }) => {
            assert_eq!(record_identifier, RecordIdentifier(1));
            assert_eq!(database_marker.commit_sequence, CommitSequence(1));
        }
        other => panic!("expected schema-emitted RecordAccepted, got {other:?}"),
    }
}

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_daemon_rejects_invalid_input_through_schema_emitted_rejection() {
    // PATTERN: invalid Input — empty topic — fails `Entry::validate`
    // inside SignalActor::accept on the Nix-built daemon. The reply is
    // the schema-emitted `SignalRejection` variant carrying the schema-
    // emitted `ValidationError::EmptyTopic`. The CLI prints the
    // schema-emitted NOTA round-trip; we parse it back through
    // `Output::FromStr` and match the typed variant.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    // Empty topic — schema-emitted Entry validation should reject.
    let nota_input = "(Record ([] Decision [body content] Maximum))";
    let output = run_cli_for_output(&binaries, daemon.socket(), nota_input);

    assert_eq!(
        output,
        Output::Rejected(SignalRejection {
            validation_error: ValidationError::EmptyTopic,
            database_marker: marker(0, 0),
        }),
        "the Nix-built daemon's schema-emitted SignalRejection variant must arrive intact"
    );
}

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_daemon_persists_state_across_two_cli_invocations() {
    // PATTERN: two CLI binary invocations against the SAME daemon
    // process show the schema-emitted DatabaseMarker.commit_sequence
    // advancing monotonically. The SEMA store is single-writer per
    // record 949; this test proves the daemon's durable `.sema` store
    // is shared between connections. Restart persistence is covered by
    // `daemon_persists_sema_file_across_a_restart`.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    let first = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([[nix-integration]] Decision [first commit] Maximum))",
    );
    let first_marker = match first {
        Output::RecordAccepted(receipt) => receipt.database_marker,
        other => panic!("expected RecordAccepted, got {other:?}"),
    };

    let second = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([[nix-integration]] Decision [second commit] Maximum))",
    );
    let second_marker = match second {
        Output::RecordAccepted(receipt) => receipt.database_marker,
        other => panic!("expected RecordAccepted, got {other:?}"),
    };

    assert!(
        second_marker.commit_sequence.0 > first_marker.commit_sequence.0,
        "schema-emitted CommitSequence advances across CLI invocations: {} -> {}",
        first_marker.commit_sequence.0,
        second_marker.commit_sequence.0
    );
    // The state digest also evolves (different records contribute
    // different magnitude weights into the digest fold).
    assert_ne!(
        first_marker.state_digest, second_marker.state_digest,
        "schema-emitted StateDigest must reflect the new record"
    );
}

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_daemon_observes_recorded_entries_back_through_query() {
    // PATTERN: Record then Observe through the Nix-built binaries. The
    // Observe path crosses Signal → Nexus → SEMA → Nexus → Signal end-
    // to-end, producing the schema-emitted `RecordsObserved` variant
    // with the original Entry echoed inside `RecordSet`.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    let _recorded = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([[nix-integration]] Decision [observe round trip] Maximum))",
    );

    let observed = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Observe ((Full [[nix-integration]]) (Some Decision)))",
    );

    match observed {
        Output::RecordsObserved(records) => {
            // The schema-emitted RecordSet carries the typed Entry; we
            // assert against schema-typed fixture (record 995).
            assert_eq!(
                records.record_set.0,
                vec![entry("observe round trip")],
                "RecordsObserved must echo the schema-emitted Entry we recorded"
            );
            assert_eq!(
                records.database_marker.commit_sequence,
                CommitSequence(1),
                "Observe does not advance the schema-emitted CommitSequence"
            );
        }
        other => panic!("expected schema-emitted RecordsObserved, got {other:?}"),
    }
}

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_daemon_returns_missed_when_no_matching_record_exists() {
    // PATTERN: Observe against an empty store traverses to the SEMA
    // plane and returns `SemaOutput::Missed`, which lowers through the
    // Nexus reverse plane to `Output::Error(ErrorReport)`. The CLI
    // prints the NOTA, we parse it back through the schema-emitted
    // FromStr surface.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    let output = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Observe ((Full [[no-such-topic]]) (Some Decision)))",
    );

    match output {
        Output::Error(ErrorReport {
            error_message,
            database_marker,
        }) => {
            // The schema-emitted ErrorMessage carries the SEMA "no matching record" string.
            assert_eq!(error_message.0, "no matching record");
            assert_eq!(database_marker, marker(0, 0));
        }
        other => panic!("expected schema-emitted Output::Error, got {other:?}"),
    }
}

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_daemon_handles_back_to_back_inputs_through_one_socket() {
    // PATTERN: multiple sequential inputs against ONE daemon process
    // exercise the daemon's accept-loop. Each input arrives on a fresh
    // Unix-socket connection; the daemon's `handle_stream` consumes
    // one input + writes one output per stream. The schema-emitted
    // CommitSequence advances by 3 across the burst.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    let descriptions = ["alpha", "beta", "gamma"];
    let mut markers = Vec::new();

    for description in descriptions {
        let nota_input = format!("(Record ([[nix-integration]] Decision [{description}] Maximum))");
        let output = run_cli_for_output(&binaries, daemon.socket(), &nota_input);
        match output {
            Output::RecordAccepted(receipt) => markers.push(receipt.database_marker),
            other => panic!("expected RecordAccepted for {description}, got {other:?}"),
        }
    }

    assert_eq!(markers.len(), 3);
    assert_eq!(markers[0].commit_sequence, CommitSequence(1));
    assert_eq!(markers[1].commit_sequence, CommitSequence(2));
    assert_eq!(markers[2].commit_sequence, CommitSequence(3));
    // Each digest distinct — proving Record actually changed state, not just the counter.
    let digests: Vec<_> = markers.iter().map(|m| m.state_digest.0).collect();
    let mut sorted = digests.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        3,
        "schema-emitted StateDigest changes per record"
    );
}

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_binaries_carry_schema_emitted_round_trip_for_every_output_variant() {
    // PATTERN: drive each schema-emitted Output variant through the
    // Nix-built binaries; parse the CLI's stdout back through the
    // schema-emitted Output::FromStr. The test proves the NOTA wire
    // form and the FromStr surface stay in sync for every variant —
    // a regression here would mean a CLI user sees output the CLI
    // itself cannot parse, which would silently break tooling.
    //
    // Variants exercised: RecordAccepted (happy Record), RecordRemoved
    // (SEMA remove), Rejected (Signal validation), Error (SEMA missed),
    // RecordsObserved (after Record + Observe). Five CLI invocations, five typed
    // assertions, all parsed through `Output::from_str`.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    // Variant 1: RecordAccepted.
    let recorded = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([[nix-integration]] Decision [variant tour] Maximum))",
    );
    assert!(matches!(recorded, Output::RecordAccepted(_)));
    assert_eq!(
        recorded.route(),
        OutputRoute::RecordAccepted,
        "schema-emitted OutputRoute round-trips through CLI stdout"
    );

    // Variant 2: RecordRemoved.
    let removed = run_cli_for_output(&binaries, daemon.socket(), "(Remove 1)");
    assert!(matches!(removed, Output::RecordRemoved(_)));
    assert_eq!(removed.route(), OutputRoute::RecordRemoved);

    let rerecorded = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([[nix-integration]] Decision [variant tour] Maximum))",
    );
    assert!(matches!(rerecorded, Output::RecordAccepted(_)));

    // Variant 3: Rejected (Signal validation).
    let rejected = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([] Decision [empty topic] Maximum))",
    );
    assert!(matches!(rejected, Output::Rejected(_)));
    assert_eq!(rejected.route(), OutputRoute::Rejected);

    // Variant 4: Error (SEMA missed).
    let errored = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Observe ((Full [[no-such-topic]]) (Some Decision)))",
    );
    assert!(matches!(errored, Output::Error(_)));
    assert_eq!(errored.route(), OutputRoute::Error);

    // Variant 5: RecordsObserved.
    let observed = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Observe ((Full [[nix-integration]]) (Some Decision)))",
    );
    assert!(matches!(observed, Output::RecordsObserved(_)));
    assert_eq!(observed.route(), OutputRoute::RecordsObserved);
}

// ---------------------------------------------------------------------------
// Concurrency probe — the daemon serves ONE input per stream but the
// listener accepts many streams. We don't push to true parallelism here
// (one-shot per stream is the current contract), but we DO verify that
// sequential connections from different invocations alias to the same
// durable `.sema` store handle.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_daemon_alias_state_across_separate_cli_processes() {
    // PATTERN: two SEPARATE CLI processes (so two distinct exec calls)
    // hit the same daemon process. State must alias between them.
    // Distinct from the persistence test in that this measures process-
    // boundary, not connection-reuse.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    // Independent processes — exec a fresh CLI binary each time.
    let mut child_a = Command::new(&binaries.spirit_cli)
        .arg("(Record ([[nix-integration]] Decision [process a record] Maximum))")
        .env("SPIRIT_NEXT_SOCKET", daemon.socket())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn CLI process a");
    let exit_a = child_a.wait().expect("CLI process a wait");
    assert!(exit_a.success(), "CLI process a failed");
    let mut stdout_a = String::new();
    child_a
        .stdout
        .take()
        .expect("CLI a stdout")
        .read_to_string(&mut stdout_a)
        .expect("read CLI a stdout");

    let observed = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Observe ((Full [[nix-integration]]) (Some Decision)))",
    );
    assert!(
        matches!(observed, Output::RecordsObserved(_)),
        "the daemon must remember the record across separate CLI processes"
    );
}

// ---------------------------------------------------------------------------
// Platform helpers.
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    true
}
