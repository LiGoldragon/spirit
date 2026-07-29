mod support;

use std::{
    fs,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    process::Command,
    thread,
    time::Duration,
};

use support::process::{
    ArchiveSource, CandidateCopyError, CandidateStoreCopy, CandidateStoreSources, CommandIsolation,
    ManagedChild, ManualSourceConfigurationError, ProcessSandbox, StoreFileFingerprint,
};

const ENVIRONMENT_PROBE: &str = "SPIRIT_PROCESS_ENVIRONMENT_PROBE";
const CHILD_LIFETIME_PROBE: &str = "SPIRIT_PROCESS_CHILD_LIFETIME_PROBE";

#[test]
fn sandbox_contains_every_process_artifact_and_cleans_it_automatically() {
    let root = {
        let sandbox = ProcessSandbox::new("artifact-layout").expect("create process sandbox");
        let root = sandbox.root().to_path_buf();
        for path in [
            sandbox.working_socket(),
            sandbox.meta_socket(),
            sandbox.configuration(),
            sandbox.live_database(),
            sandbox.archive_database(),
            sandbox.build_directory(),
        ] {
            assert!(
                path.starts_with(&root),
                "process artifact escaped sandbox: {}",
                path.display()
            );
        }
        fs::create_dir(sandbox.build_directory()).expect("create sandbox build directory");
        fs::write(sandbox.configuration(), b"synthetic configuration")
            .expect("write sandbox configuration");
        root
    };

    assert!(
        !root.exists(),
        "dropping the default sandbox removes every process artifact"
    );
}

#[test]
fn missing_manual_source_configuration_is_a_typed_error() {
    assert!(matches!(
        CandidateStoreSources::require_values(None, None),
        Err(ManualSourceConfigurationError::MissingLive)
    ));
    assert!(matches!(
        CandidateStoreSources::require_values(Some("/tmp/live.sema".into()), None),
        Err(ManualSourceConfigurationError::MissingArchiveChoice)
    ));
    assert!(matches!(
        CandidateStoreSources::require_values(
            Some("relative-live.sema".into()),
            Some("absent".into())
        ),
        Err(ManualSourceConfigurationError::NonAbsoluteLive(_))
    ));
}

#[test]
fn disposable_live_and_archive_sources_copy_independently_and_stay_unchanged() {
    let sources = tempfile::tempdir().expect("create disposable source directory");
    let live = sources.path().join("source-live.sema");
    let archive = sources.path().join("source-archive.sema");
    fs::write(&live, b"synthetic live bytes").expect("write synthetic live source");
    fs::write(&archive, b"synthetic archive bytes").expect("write synthetic archive source");
    fs::set_permissions(&live, fs::Permissions::from_mode(0o640))
        .expect("set synthetic live permissions");
    fs::set_permissions(&archive, fs::Permissions::from_mode(0o600))
        .expect("set synthetic archive permissions");
    let live_before = StoreFileFingerprint::capture(&live).expect("fingerprint live source");
    let archive_before =
        StoreFileFingerprint::capture(&archive).expect("fingerprint archive source");

    let copy = CandidateStoreCopy::prepare(CandidateStoreSources::new(
        &live,
        ArchiveSource::File(archive.clone()),
    ))
    .expect("prepare isolated candidate copies");
    let candidate_root = copy.root().to_path_buf();
    let candidate_live = copy.live_database().to_path_buf();
    let candidate_archive = copy
        .archive_database()
        .expect("archive source was explicit")
        .to_path_buf();
    assert!(candidate_live.starts_with(&candidate_root));
    assert!(candidate_archive.starts_with(&candidate_root));

    fs::write(&candidate_live, b"candidate-only mutation")
        .expect("mutate disposable candidate live file");
    fs::write(&candidate_archive, b"candidate-only archive mutation")
        .expect("mutate disposable candidate archive file");
    copy.assert_sources_unchanged()
        .expect("candidate exercise leaves source files unchanged");
    assert_eq!(
        StoreFileFingerprint::capture(&live).expect("refingerprint live source"),
        live_before
    );
    assert_eq!(
        StoreFileFingerprint::capture(&archive).expect("refingerprint archive source"),
        archive_before
    );

    drop(copy);
    assert!(
        !candidate_root.exists(),
        "candidate live/archive copies are auto-cleaned"
    );
}

#[test]
fn absent_archive_is_explicit_and_no_sibling_is_inferred() {
    let sources = tempfile::tempdir().expect("create disposable source directory");
    let live = sources.path().join("source-live.sema");
    let sibling = sources.path().join("source-live.archive.sema");
    fs::write(&live, b"synthetic live bytes").expect("write synthetic live source");
    fs::write(&sibling, b"must not be inferred").expect("write unrelated sibling");

    let copy =
        CandidateStoreCopy::prepare(CandidateStoreSources::new(&live, ArchiveSource::Absent))
            .expect("prepare explicit live-only candidate");

    assert!(
        copy.archive_database().is_none(),
        "an unrelated sibling is not treated as an archive input"
    );
}

#[test]
fn symlink_source_is_refused_before_copy() {
    let sources = tempfile::tempdir().expect("create disposable source directory");
    let live = sources.path().join("source-live.sema");
    let symlink = sources.path().join("source-link.sema");
    fs::write(&live, b"synthetic live bytes").expect("write synthetic live source");
    std::os::unix::fs::symlink(&live, &symlink).expect("create disposable symlink");

    assert!(matches!(
        CandidateStoreCopy::prepare(CandidateStoreSources::new(
            &symlink,
            ArchiveSource::Absent
        )),
        Err(CandidateCopyError::Symlink(path)) if path == symlink
    ));
}

#[test]
fn isolated_command_exposes_only_its_explicit_environment() {
    assert!(
        std::env::var_os("PATH").is_some(),
        "the parent test process supplies PATH for this falsification"
    );
    let mut command = Command::isolated(std::env::current_exe().expect("current test executable"));
    command
        .args([
            "--ignored",
            "--exact",
            "environment_probe_observes_explicit_environment_only",
            "--nocapture",
        ])
        .env(ENVIRONMENT_PROBE, "explicit");
    let output = command.output().expect("run isolated environment probe");

    assert!(
        output.status.success(),
        "environment probe stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("environment is isolated"),
        "the nested probe executed its isolated branch"
    );
}

#[test]
fn managed_child_is_reaped_and_socket_readiness_requires_a_socket() {
    let sandbox = ProcessSandbox::new("managed-child").expect("create process sandbox");
    let listener =
        UnixListener::bind(sandbox.working_socket()).expect("bind disposable readiness socket");
    let mut command = Command::isolated(std::env::current_exe().expect("current test executable"));
    command
        .args([
            "--ignored",
            "--exact",
            "managed_child_lifetime_probe",
            "--nocapture",
        ])
        .env(CHILD_LIFETIME_PROBE, "explicit");
    let mut child = ManagedChild::spawn(&mut command, "child lifetime probe")
        .expect("spawn managed child probe");

    child
        .wait_for_unix_socket(&sandbox.working_socket(), Duration::from_secs(2))
        .expect("actual Unix socket becomes ready");
    assert!(
        child.try_wait().expect("inspect child").is_none(),
        "probe remains alive before explicit termination"
    );
    let status = child
        .terminate()
        .expect("terminate and reap child")
        .expect("child had not already been consumed");
    assert!(
        !status.success(),
        "terminating the long-running probe returns its terminal status"
    );
    assert!(
        child
            .terminate()
            .expect("repeat termination is idempotent")
            .is_none(),
        "a reaped child has no second terminal wait"
    );
    drop(listener);
}

#[test]
#[ignore = "child-only probe invoked by the isolated-command witness"]
fn environment_probe_observes_explicit_environment_only() {
    assert_eq!(std::env::var(ENVIRONMENT_PROBE).as_deref(), Ok("explicit"));
    assert!(
        std::env::var_os("PATH").is_none(),
        "the child must not inherit PATH"
    );
    assert!(
        std::env::var_os("HOME").is_none(),
        "the child must not inherit HOME"
    );
    println!("environment is isolated");
}

#[test]
#[ignore = "child-only probe invoked by the managed-child witness"]
fn managed_child_lifetime_probe() {
    assert_eq!(
        std::env::var(CHILD_LIFETIME_PROBE).as_deref(),
        Ok("explicit")
    );
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
