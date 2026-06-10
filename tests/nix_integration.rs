//! Nix-driven integration tests for the spirit schema-driven stack.
//!
//! These tests are the cycle-3-prep forcing function for record 1006
//! (Maximum, 2026-05-27): tests must PROVE not pretend; the canonical
//! shape is schema files driving Nix-built binaries that the test
//! launches and exchanges real rkyv signal frames with over a real
//! Unix socket. Every component — `spirit` (CLI), `spirit-
//! daemon` (daemon, holds the SignalAdmission + Engine + Store triad) — is
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
//!      test, or via `SPIRIT_NIX_BUILD_RESULT` if pre-built).
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
//! The script seeds `SPIRIT_NIX_BUILD_RESULT` with a pre-built
//! binary directory so the test bypasses its own `nix build`.
//!
//! ## What each test proves
//!
//! - `nix_built_spirit_cli_records_through_real_socket_to_nix_built_daemon`
//!   — happy-path Record traverses CLI binary → Unix socket → daemon
//!   binary → SignalAdmission → Engine → Store, returning `RecordAccepted`.
//!   The test parses the CLI's stdout back through the schema-emitted
//!   `Output::FromStr` and matches typed variants.
//!
//! - `nix_built_daemon_rejects_invalid_input_through_schema_emitted_rejection`
//!   — invalid Input is rejected by `SignalAdmission::admit` and the
//!   schema-emitted `SignalRejection` flows back to the CLI through
//!   the rkyv signal frame.
//!
//! - `nix_built_daemon_persists_state_across_two_cli_invocations`
//!   — two CLI invocations against the same daemon process show
//!   monotonic `CommitSequence` advancement on the schema-emitted
//!   `VersionReported` after writes.
//!
//! - `nix_built_daemon_observes_recorded_entries_back_through_query`
//!   — a Record followed by an Observe Query returns the schema-emitted
//!   `RecordsStashed` variant, then `LookupStash` returns the full
//!   `RecordsObserved` payload.
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
//!   — sanity-check the Nix build produces both `bin/spirit` and
//!   `bin/spirit-daemon`, proving the schema-driven build pipeline
//!   reaches the binary stage.
//!
//! - `nix_built_binaries_round_trip_representative_schema_outputs`
//!   — drive representative schema-emitted `Output` variants, including
//!   `VersionReported`, through the CLI and parse stdout back through the
//!   schema-emitted `Output::FromStr`, proving the NOTA form and the
//!   FromStr surface stay in sync.

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

use spirit::Configuration;
use spirit::schema::signal::{
    Categories, DatabaseMarker, Description, Entry, Kind, Magnitude, Output, OutputRoute, Privacy,
    RecordIdentifier, SignalRejection, ValidationError,
};
use tempfile::TempDir;

fn assert_short_record_identifier(identifier: &RecordIdentifier) {
    assert!(
        (4..=7).contains(&identifier.payload().len()),
        "record identifier should use a four-to-seven-character code: {:?}",
        identifier.payload()
    );
    assert!(
        identifier
            .payload()
            .chars()
            .all(|character| character.is_ascii_digit() || character.is_ascii_lowercase()),
        "record identifier should be lower-base36: {:?}",
        identifier.payload()
    );
}

fn record_identifier_argument(identifier: &RecordIdentifier) -> String {
    identifier.payload().to_owned()
}

// ---------------------------------------------------------------------------
// Nix-build harness: locate the schema-driven binaries.
// ---------------------------------------------------------------------------

/// Outcome of locating Nix-built spirit binaries.
///
/// Tests call `NixBuiltBinaries::ensure()` which either reuses
/// `SPIRIT_NIX_BUILD_RESULT` (a pre-built `result/` directory
/// containing `bin/spirit` + `bin/spirit-daemon`) or invokes
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
        if let Ok(directory) = env::var("SPIRIT_NIX_BUILD_RESULT") {
            return Self::from_directory(&PathBuf::from(directory));
        }
        Self::from_directory(&Self::nix_build())
    }

    fn from_directory(directory: &Path) -> Self {
        let spirit_cli = directory.join("bin").join("spirit");
        let spirit_daemon = directory.join("bin").join("spirit-daemon");
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

    /// Run `nix build` against the workspace flake. Present local dependency
    /// checkouts are passed as `path:` overrides so in-flight stack work is
    /// exercised; missing default checkouts are skipped so a clean machine can
    /// still verify the packaged binary against the flake pins.
    fn nix_build() -> PathBuf {
        let repo_root = repo_root();
        let temp_link = repo_root.join("target").join("nix-integration-result");
        if temp_link.exists() {
            let _ = std::fs::remove_file(&temp_link);
        }

        let mut command = Command::new("nix");
        command
            .arg("build")
            .arg("--log-format")
            .arg("bar-with-logs")
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
    let workspace_default = |name: &str, env_key: &str| -> Option<String> {
        match env::var(env_key) {
            Ok(path) => Some(path),
            Err(_) => {
                let path = PathBuf::from(format!("/git/github.com/LiGoldragon/{name}"));
                path.exists().then(|| path.display().to_string())
            }
        }
    };
    vec![
        (
            "nota-next-source",
            workspace_default("nota-next", "NOTA_NEXT_PATH"),
        ),
        (
            "nota-codec-source",
            workspace_default("nota-codec", "NOTA_CODEC_PATH"),
        ),
        (
            "nota-derive-source",
            workspace_default("nota-derive", "NOTA_DERIVE_PATH"),
        ),
        ("schema-source", workspace_default("schema", "SCHEMA_PATH")),
        (
            "schema-next-source",
            workspace_default("schema-next", "SCHEMA_NEXT_PATH"),
        ),
        (
            "schema-rust-next-source",
            workspace_default("schema-rust-next", "SCHEMA_RUST_NEXT_PATH"),
        ),
        ("sema-source", workspace_default("sema", "SEMA_PATH")),
        (
            "sema-engine-source",
            workspace_default("sema-engine", "SEMA_ENGINE_PATH"),
        ),
        (
            "signal-core-source",
            workspace_default("signal-core", "SIGNAL_CORE_PATH"),
        ),
        (
            "signal-frame-source",
            workspace_default("signal-frame", "SIGNAL_FRAME_PATH"),
        ),
        (
            "signal-sema-source",
            workspace_default("signal-sema", "SIGNAL_SEMA_PATH"),
        ),
        (
            "triad-runtime-source",
            workspace_default("triad-runtime", "TRIAD_RUNTIME_PATH"),
        ),
    ]
    .into_iter()
    .filter_map(|(input, path)| path.map(|path| (input, format!("path:{path}"))))
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
        let socket_path = temp_directory.path().join("spirit.sock");
        let database_path = temp_directory.path().join("spirit.sema");
        let configuration_path = temp_directory.path().join("spirit.config.rkyv");
        Configuration::new(&socket_path, &database_path)
            .write_binary_file(&configuration_path)
            .expect("write binary daemon configuration");

        let child = Command::new(&binaries.spirit_daemon)
            .arg(configuration_path)
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
        .env("SPIRIT_SOCKET", socket)
        .output()
        .expect("run CLI");
    assert!(
        output.status.success(),
        "spirit CLI failed (status {}): stderr={}; stdout={}",
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
        categories: Categories::from_strings(vec![String::from("nix-integration")]),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: Magnitude::Maximum.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
    }
}

fn marker(commit_sequence: u64, state_digest: u64) -> DatabaseMarker {
    DatabaseMarker {
        commit_sequence: commit_sequence.into(),
        state_digest: state_digest.into(),
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
    // sends an rkyv-encoded signal frame, the daemon's SignalAdmission +
    // Engine + Store triad runs, the schema-emitted record identifier comes
    // back through the reverse plane chain, the CLI writes the NOTA
    // round-trip to stdout, and we parse it back into the schema-
    // emitted Output enum.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    let nota_input = "(Record ([Sustaining] Decision [end to end through nix built binaries] Maximum Minimum Zero))";
    let output = run_cli_for_output(&binaries, daemon.socket(), nota_input);

    // SCHEMA-TYPED ASSERTION: not on the string, on the parsed schema-emitted variant.
    match output {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected schema-emitted RecordAccepted, got {other:?}"),
    }
}

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_daemon_rejects_invalid_input_through_schema_emitted_rejection() {
    // PATTERN: invalid Input — empty category — fails `Entry::validate`
    // inside SignalAdmission::admit on the Nix-built daemon. The reply is
    // the schema-emitted `SignalRejection` variant carrying the schema-
    // emitted `ValidationError::EmptyCategory`. The CLI prints the
    // schema-emitted NOTA round-trip; we parse it back through
    // `Output::FromStr` and match the typed variant.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    // Empty category — schema-emitted Entry validation should reject.
    let nota_input = "(Record ([] Decision [body content] Maximum Minimum Zero))";
    let output = run_cli_for_output(&binaries, daemon.socket(), nota_input);

    assert_eq!(
        output,
        Output::rejected(SignalRejection {
            validation_error: ValidationError::EmptyCategory,
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
        "(Record ([Sustaining] Decision [first commit] Maximum Minimum Zero))",
    );
    match first {
        Output::RecordAccepted(receipt) => assert_short_record_identifier(receipt.payload()),
        other => panic!("expected RecordAccepted, got {other:?}"),
    };
    let first_marker = match run_cli_for_output(&binaries, daemon.socket(), "Version") {
        Output::VersionReported(report) => report.payload().database_marker.clone(),
        other => panic!("expected VersionReported after first record, got {other:?}"),
    };

    let second = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([Sustaining] Decision [second commit] Maximum Minimum Zero))",
    );
    match second {
        Output::RecordAccepted(receipt) => assert_short_record_identifier(receipt.payload()),
        other => panic!("expected RecordAccepted, got {other:?}"),
    };
    let second_marker = match run_cli_for_output(&binaries, daemon.socket(), "Version") {
        Output::VersionReported(report) => report.payload().database_marker.clone(),
        other => panic!("expected VersionReported after second record, got {other:?}"),
    };

    assert!(
        second_marker.commit_sequence > first_marker.commit_sequence,
        "schema-emitted CommitSequence advances across CLI invocations: {} -> {}",
        first_marker.commit_sequence,
        second_marker.commit_sequence
    );
    // The state digest also evolves (different records contribute
    // different magnitude importances into the digest fold).
    assert_ne!(
        first_marker.state_digest, second_marker.state_digest,
        "schema-emitted StateDigest must reflect the new record"
    );
}

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_daemon_observes_recorded_entries_back_through_query() {
    // PATTERN: Record then Observe through the Nix-built binaries. The
    // Observe path crosses Signal -> Nexus -> SEMA -> Nexus -> Signal
    // end-to-end, producing the schema-emitted `RecordsStashed` variant.
    // A follow-up LookupStash returns the original Entry inside
    // `RecordsObserved`.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    let _recorded = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([Sustaining] Decision [observe round trip] Maximum Minimum Zero))",
    );

    let observed = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Observe ((Full [Sustaining]) (Some Decision) (Exact Zero)))",
    );

    let stash_handle = match observed {
        Output::RecordsStashed(stashed) => {
            assert_eq!(stashed.record_count, 1);
            assert_eq!(
                stashed.database_marker.commit_sequence, 1,
                "Observe does not advance the schema-emitted CommitSequence"
            );
            stashed.stash_handle.clone()
        }
        other => panic!("expected schema-emitted RecordsStashed, got {other:?}"),
    };

    let resolved = run_cli_for_output(
        &binaries,
        daemon.socket(),
        &format!("(LookupStash {stash_handle})"),
    );

    match resolved {
        Output::RecordsObserved(records) => {
            assert_short_record_identifier(&records.record_set[0].record_identifier);
            assert_eq!(
                records.record_set[0].entry,
                entry("observe round trip"),
                "LookupStash must echo the schema-emitted Entry we recorded"
            );
            assert_eq!(records.database_marker.commit_sequence, 1);
        }
        other => panic!("expected schema-emitted RecordsObserved from LookupStash, got {other:?}"),
    }
}

#[test]
#[ignore = "invokes nix build; run via cargo test --test nix_integration -- --ignored"]
fn nix_built_daemon_returns_missed_when_no_matching_record_exists() {
    // PATTERN: Observe against an empty store traverses to the SEMA read
    // plane and returns `SemaReadOutput::Missed`, which lowers through
    // the Nexus reverse plane to `Output::Error(ErrorReport)`. The CLI
    // prints the NOTA, we parse it back through the schema-emitted
    // FromStr surface.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    let output = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Observe ((Full [no-such-category]) (Some Decision) (Exact Zero)))",
    );

    match output {
        Output::Error(report) => {
            // The schema-emitted ErrorMessage carries the SEMA "no matching record" string.
            assert_eq!(report.payload().error_message, "no matching record");
            assert_eq!(report.payload().database_marker, marker(0, 0));
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
        let nota_input =
            format!("(Record ([Sustaining] Decision [{description}] Maximum Minimum Zero))");
        let output = run_cli_for_output(&binaries, daemon.socket(), &nota_input);
        match output {
            Output::RecordAccepted(receipt) => assert_short_record_identifier(receipt.payload()),
            other => panic!("expected RecordAccepted for {description}, got {other:?}"),
        }
        match run_cli_for_output(&binaries, daemon.socket(), "Version") {
            Output::VersionReported(report) => {
                markers.push(report.payload().database_marker.clone())
            }
            other => panic!("expected VersionReported for {description}, got {other:?}"),
        }
    }

    assert_eq!(markers.len(), 3);
    assert_eq!(markers[0].commit_sequence, 1);
    assert_eq!(markers[1].commit_sequence, 2);
    assert_eq!(markers[2].commit_sequence, 3);
    // Each digest distinct — proving Record actually changed state, not just the counter.
    let digests: Vec<_> = markers.iter().map(|m| m.state_digest.clone()).collect();
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
fn nix_built_binaries_round_trip_representative_schema_outputs() {
    // PATTERN: drive each schema-emitted Output variant through the
    // Nix-built binaries; parse the CLI's stdout back through the
    // schema-emitted Output::FromStr. The test proves the NOTA wire
    // form and the FromStr surface stay in sync for every variant —
    // a regression here would mean a CLI user sees output the CLI
    // itself cannot parse, which would silently break tooling.
    //
    // Variants exercised: VersionReported (component version),
    // RecordAccepted (happy Record), RecordRemoved (SEMA remove),
    // CertaintyChanged (SEMA mutate), Rejected (Signal validation), Error
    // (SEMA missed), RecordsStashed (after Record + Observe). Seven typed
    // assertions, all parsed through `Output::from_str`.
    let binaries = NixBuiltBinaries::ensure();
    let daemon = DaemonProcess::spawn(&binaries);

    // Variant 1: VersionReported.
    let version = run_cli_for_output(&binaries, daemon.socket(), "Version");
    match &version {
        Output::VersionReported(report) => {
            assert_eq!(
                report.payload().version_text.payload(),
                env!("CARGO_PKG_VERSION")
            );
            assert_eq!(report.payload().database_marker, marker(0, 0));
        }
        other => panic!("expected VersionReported, got {other:?}"),
    }
    assert_eq!(version.route(), OutputRoute::VersionReported);

    // Variant 2: RecordAccepted.
    let recorded = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([Sustaining] Decision [variant tour] Maximum Minimum Zero))",
    );
    let recorded_identifier = match &recorded {
        Output::RecordAccepted(receipt) => receipt.payload().clone(),
        other => panic!("expected RecordAccepted, got {other:?}"),
    };
    assert_short_record_identifier(&recorded_identifier);
    assert_eq!(
        recorded.route(),
        OutputRoute::RecordAccepted,
        "schema-emitted OutputRoute round-trips through CLI stdout"
    );

    // Variant 3: RecordRemoved.
    let removed = run_cli_for_output(
        &binaries,
        daemon.socket(),
        &format!(
            "(Remove {})",
            record_identifier_argument(&recorded_identifier)
        ),
    );
    assert!(matches!(removed, Output::RecordRemoved(_)));
    assert_eq!(removed.route(), OutputRoute::RecordRemoved);

    let rerecorded = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([Sustaining] Decision [variant tour] Maximum Minimum Zero))",
    );
    let rerecorded_identifier = match &rerecorded {
        Output::RecordAccepted(receipt) => receipt.payload().clone(),
        other => panic!("expected second RecordAccepted, got {other:?}"),
    };
    assert_short_record_identifier(&rerecorded_identifier);

    // Variant 4: CertaintyChanged.
    let changed = run_cli_for_output(
        &binaries,
        daemon.socket(),
        &format!(
            "(ChangeCertainty ({} Zero))",
            record_identifier_argument(&rerecorded_identifier)
        ),
    );
    assert!(matches!(changed, Output::CertaintyChanged(_)));
    assert_eq!(changed.route(), OutputRoute::CertaintyChanged);

    // Variant 5: Rejected (Signal validation).
    let rejected = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Record ([] Decision [empty category] Maximum Minimum Zero))",
    );
    assert!(matches!(rejected, Output::Rejected(_)));
    assert_eq!(rejected.route(), OutputRoute::Rejected);

    // Variant 6: Error (SEMA missed).
    let errored = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Observe ((Full [no-such-category]) (Some Decision) (Exact Zero)))",
    );
    assert!(matches!(errored, Output::Error(_)));
    assert_eq!(errored.route(), OutputRoute::Error);

    // Variant 7: RecordsStashed.
    let observed = run_cli_for_output(
        &binaries,
        daemon.socket(),
        "(Observe ((Full [Sustaining]) (Some Decision) (Exact Zero)))",
    );
    assert!(matches!(observed, Output::RecordsStashed(_)));
    assert_eq!(observed.route(), OutputRoute::RecordsStashed);
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
        .arg("(Record ([Sustaining] Decision [process a record] Maximum Minimum Zero))")
        .env("SPIRIT_SOCKET", daemon.socket())
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
        "(Observe ((Full [Sustaining]) (Some Decision) (Exact Zero)))",
    );
    let stash_handle = match observed {
        Output::RecordsStashed(stashed) => {
            assert_eq!(stashed.record_count, 1);
            stashed.stash_handle.clone()
        }
        other => panic!("expected RecordsStashed across separate CLI processes, got {other:?}"),
    };
    let resolved = run_cli_for_output(
        &binaries,
        daemon.socket(),
        &format!("(LookupStash {stash_handle})"),
    );
    match resolved {
        Output::RecordsObserved(records) => {
            assert_short_record_identifier(&records.record_set[0].record_identifier);
            assert_eq!(
                records.record_set[0].entry,
                entry("process a record"),
                "the daemon must remember the record across separate CLI processes"
            )
        }
        other => panic!("expected RecordsObserved from LookupStash, got {other:?}"),
    }
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
