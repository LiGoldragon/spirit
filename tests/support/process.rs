use std::{
    ffi::{OsStr, OsString},
    fs, io,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

use tempfile::{Builder, TempDir};

/// Construct child commands with no inherited process environment.
///
/// Callers add only the concrete nonsecret variables required by the child,
/// such as one sandbox-owned socket path. Toolchain commands must likewise
/// restore an explicit reviewed set instead of inheriting the agent process.
pub trait CommandIsolation {
    /// Create a command whose environment starts empty.
    fn isolated(program: impl AsRef<OsStr>) -> Self;

    /// Restore one reviewed nonsecret parent variable needed to locate a
    /// toolchain program.
    fn restore_nonsecret(
        &mut self,
        variable: NonSecretEnvironmentVariable,
    ) -> io::Result<&mut Self>;

    /// Restore one reviewed variable when the parent toolchain supplied it.
    fn restore_nonsecret_if_present(
        &mut self,
        variable: NonSecretEnvironmentVariable,
    ) -> io::Result<&mut Self>;
}

impl CommandIsolation for Command {
    fn isolated(program: impl AsRef<OsStr>) -> Self {
        let mut command = Self::new(program);
        command.env_clear();
        command
    }

    fn restore_nonsecret(
        &mut self,
        variable: NonSecretEnvironmentVariable,
    ) -> io::Result<&mut Self> {
        let name = variable.name();
        let value = variable.validated_parent_value()?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("required nonsecret environment variable {name} is absent"),
            )
        })?;
        Ok(self.env(name, value))
    }

    fn restore_nonsecret_if_present(
        &mut self,
        variable: NonSecretEnvironmentVariable,
    ) -> io::Result<&mut Self> {
        if let Some(value) = variable.validated_parent_value()? {
            self.env(variable.name(), value);
        }
        Ok(self)
    }
}

/// Closed parent-environment allowlist for test toolchain operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NonSecretEnvironmentVariable {
    /// Executable search path.
    Path,
    /// Nix's ephemeral Cargo root containing its vendored-source map.
    NixCargoHome,
}

impl NonSecretEnvironmentVariable {
    const fn name(self) -> &'static str {
        match self {
            Self::Path => "PATH",
            Self::NixCargoHome => "CARGO_HOME",
        }
    }

    fn validated_parent_value(self) -> io::Result<Option<OsString>> {
        let Some(value) = std::env::var_os(self.name()) else {
            return Ok(None);
        };
        if self == Self::NixCargoHome {
            let build_root = std::env::var_os("NIX_BUILD_TOP").ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "CARGO_HOME is not restored outside an ephemeral Nix build",
                )
            })?;
            let cargo_home = Path::new(&value);
            let build_root = Path::new(&build_root);
            if !cargo_home.is_absolute()
                || !build_root.is_absolute()
                || !cargo_home.starts_with(build_root)
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "CARGO_HOME is not contained by NIX_BUILD_TOP",
                ));
            }
        }
        Ok(Some(value))
    }
}

/// A child process whose terminal wait is guaranteed before the handle drops.
pub struct ManagedChild {
    child: Option<Child>,
    label: &'static str,
}

impl ManagedChild {
    /// Spawn one already-isolated command.
    pub fn spawn(command: &mut Command, label: &'static str) -> io::Result<Self> {
        Ok(Self {
            child: Some(command.spawn()?),
            label,
        })
    }

    /// Take the child's piped stdout exactly once.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.as_mut()?.stdout.take()
    }

    /// Report whether the child has already exited.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child
            .as_mut()
            .map_or(Ok(None), std::process::Child::try_wait)
    }

    /// Wait for natural completion while retaining piped output handles.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "child was already consumed"))?
            .wait()
    }

    /// Kill a running child and always reap its terminal status.
    pub fn terminate(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(mut child) = self.child.take() else {
            return Ok(None);
        };
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        match child.kill() {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
            Err(error) => return Err(error),
        }
        child.wait().map(Some)
    }

    /// Wait until a path is an actual Unix socket while also watching the child.
    pub fn wait_for_unix_socket(
        &mut self,
        path: &Path,
        timeout: Duration,
    ) -> Result<(), SocketReadinessError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait()? {
                return Err(SocketReadinessError::ProcessExited {
                    label: self.label,
                    status,
                });
            }
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_socket() => return Ok(()),
                Ok(_) => {
                    return Err(SocketReadinessError::UnexpectedPath {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(SocketReadinessError::Io(error)),
            }
            if Instant::now() >= deadline {
                return Err(SocketReadinessError::Timeout {
                    label: self.label,
                    path: path.to_path_buf(),
                });
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

/// Failure while waiting for a spawned process to publish a Unix socket.
#[derive(Debug, thiserror::Error)]
pub enum SocketReadinessError {
    /// Filesystem inspection failed.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// The child exited before publishing its socket.
    #[error("{label} exited with {status} before publishing its socket")]
    ProcessExited {
        /// Human-readable process role.
        label: &'static str,
        /// Terminal child status.
        status: ExitStatus,
    },

    /// A non-socket object occupied the expected path.
    #[error("expected a Unix socket at {}, found another object", path.display())]
    UnexpectedPath {
        /// Occupied path.
        path: PathBuf,
    },

    /// The readiness deadline elapsed.
    #[error(
        "{label} did not publish its Unix socket at {} before the deadline",
        path.display()
    )]
    Timeout {
        /// Human-readable process role.
        label: &'static str,
        /// Expected socket path.
        path: PathBuf,
    },
}

/// Auto-cleaned filesystem root for one process-level test.
///
/// Every artifact accessor returns a descendant of the owned [`TempDir`].
pub struct ProcessSandbox {
    directory: TempDir,
}

impl ProcessSandbox {
    /// Create an auto-cleaned process sandbox.
    pub fn new(label: &str) -> io::Result<Self> {
        Ok(Self {
            directory: Builder::new()
                .prefix(&format!("spirit-{label}-"))
                .tempdir()?,
        })
    }

    /// Root containing every process artifact.
    pub fn root(&self) -> &Path {
        self.directory.path()
    }

    /// Ordinary working-signal socket.
    pub fn working_socket(&self) -> PathBuf {
        self.root().join("spirit.sock")
    }

    /// Owner-only meta-signal socket.
    pub fn meta_socket(&self) -> PathBuf {
        self.root().join("spirit-meta.sock")
    }

    /// Binary daemon configuration.
    pub fn configuration(&self) -> PathBuf {
        self.root().join("spirit.config.rkyv")
    }

    /// Candidate live database.
    pub fn live_database(&self) -> PathBuf {
        self.root().join("candidate-live.sema")
    }

    /// Candidate archive database.
    pub fn archive_database(&self) -> PathBuf {
        self.root().join("candidate-archive.sema")
    }

    /// Nested Cargo/Nix build artifacts.
    pub fn build_directory(&self) -> PathBuf {
        self.root().join("build")
    }

    /// Persist this sandbox after an explicit manual retention decision.
    ///
    /// Automated gates use the default drop behavior and never construct the
    /// required opt-in value.
    pub fn retain(self, _opt_in: ArtifactRetentionOptIn) -> PathBuf {
        self.directory.keep()
    }
}

/// Deliberate capability required to keep process artifacts.
pub struct ArtifactRetentionOptIn {
    _reason: String,
}

impl ArtifactRetentionOptIn {
    /// Record a non-empty manual reason for retaining artifacts.
    pub fn manual(reason: impl Into<String>) -> Result<Self, EmptyRetentionReason> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            Err(EmptyRetentionReason)
        } else {
            Ok(Self { _reason: reason })
        }
    }
}

/// Retention was requested without a manual reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("artifact retention requires a non-empty explicit manual reason")]
pub struct EmptyRetentionReason;

/// Explicit archive-source choice for a candidate copy.
pub enum ArchiveSource {
    /// The source is explicitly known to have no archive input.
    Absent,
    /// Copy this explicit archive file.
    File(PathBuf),
}

/// Explicit source files for preparing an isolated candidate.
pub struct CandidateStoreSources {
    live: PathBuf,
    archive: ArchiveSource,
}

impl CandidateStoreSources {
    /// Pair one explicit live source with an explicit archive choice.
    pub fn new(live: impl Into<PathBuf>, archive: ArchiveSource) -> Self {
        Self {
            live: live.into(),
            archive,
        }
    }

    /// Require the two manual configuration variables without inferring a
    /// sibling archive. `SPIRIT_ACCEPTANCE_SOURCE_ARCHIVE_SEMA=absent`
    /// explicitly declares that no archive input exists.
    pub fn require_manual_environment() -> Result<Self, ManualSourceConfigurationError> {
        Self::require_values(
            std::env::var_os("SPIRIT_ACCEPTANCE_SOURCE_SEMA"),
            std::env::var_os("SPIRIT_ACCEPTANCE_SOURCE_ARCHIVE_SEMA"),
        )
    }

    /// Decode already-captured configuration values.
    pub fn require_values(
        live: Option<OsString>,
        archive: Option<OsString>,
    ) -> Result<Self, ManualSourceConfigurationError> {
        let live = PathBuf::from(live.ok_or(ManualSourceConfigurationError::MissingLive)?);
        if !live.is_absolute() {
            return Err(ManualSourceConfigurationError::NonAbsoluteLive(live));
        }
        let archive = archive.ok_or(ManualSourceConfigurationError::MissingArchiveChoice)?;
        let archive = if archive == OsStr::new("absent") {
            ArchiveSource::Absent
        } else {
            let path = PathBuf::from(archive);
            if !path.is_absolute() {
                return Err(ManualSourceConfigurationError::NonAbsoluteArchive(path));
            }
            ArchiveSource::File(path)
        };
        Ok(Self::new(live, archive))
    }
}

/// Missing or unsafe manual source configuration.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ManualSourceConfigurationError {
    /// No live database was named.
    #[error("SPIRIT_ACCEPTANCE_SOURCE_SEMA must name an absolute source file")]
    MissingLive,

    /// The archive state was left implicit.
    #[error(
        "SPIRIT_ACCEPTANCE_SOURCE_ARCHIVE_SEMA must be an absolute source file or the atom absent"
    )]
    MissingArchiveChoice,

    /// The live source was not absolute.
    #[error("live source path must be absolute: {}", .0.display())]
    NonAbsoluteLive(PathBuf),

    /// The archive source was not absolute.
    #[error("archive source path must be absolute: {}", .0.display())]
    NonAbsoluteArchive(PathBuf),
}

/// Raw-file evidence captured without constructing or opening a Spirit store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreFileFingerprint {
    digest: blake3::Hash,
    length: u64,
    mode: u32,
    device: u64,
    inode: u64,
}

impl StoreFileFingerprint {
    /// Fingerprint one regular, non-symlink `.sema` file as uninterpreted bytes.
    pub fn capture(path: &Path) -> Result<Self, CandidateCopyError> {
        let metadata = fs::symlink_metadata(path).map_err(CandidateCopyError::Io)?;
        if metadata.file_type().is_symlink() {
            return Err(CandidateCopyError::Symlink(path.to_path_buf()));
        }
        if !metadata.is_file() {
            return Err(CandidateCopyError::NonRegular(path.to_path_buf()));
        }
        if path.extension() != Some(OsStr::new("sema")) {
            return Err(CandidateCopyError::WrongExtension(path.to_path_buf()));
        }
        let bytes = fs::read(path).map_err(CandidateCopyError::Io)?;
        Ok(Self {
            digest: blake3::hash(&bytes),
            length: metadata.len(),
            mode: metadata.mode(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn content_and_mode_match(&self, other: &Self) -> bool {
        self.digest == other.digest
            && self.length == other.length
            && self.mode & 0o7777 == other.mode & 0o7777
    }

    fn file_identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }
}

struct SourceEvidence {
    path: PathBuf,
    fingerprint: StoreFileFingerprint,
}

/// Independent live/archive copies contained by one auto-cleaned sandbox.
pub struct CandidateStoreCopy {
    sandbox: ProcessSandbox,
    live_source: SourceEvidence,
    archive_source: Option<SourceEvidence>,
    live_database: PathBuf,
    archive_database: Option<PathBuf>,
}

impl CandidateStoreCopy {
    /// Copy only the explicitly supplied sources into a new temporary sandbox.
    pub fn prepare(sources: CandidateStoreSources) -> Result<Self, CandidateCopyError> {
        let sandbox = ProcessSandbox::new("candidate-copy").map_err(CandidateCopyError::Io)?;
        let live_source = Self::copy_source(
            sources.live,
            sandbox.live_database(),
            "live candidate source",
        )?;
        let live_database = sandbox.live_database();
        let (archive_source, archive_database) = match sources.archive {
            ArchiveSource::Absent => (None, None),
            ArchiveSource::File(path) => {
                let candidate = sandbox.archive_database();
                let source =
                    Self::copy_source(path, candidate.clone(), "archive candidate source")?;
                (Some(source), Some(candidate))
            }
        };
        let copy = Self {
            sandbox,
            live_source,
            archive_source,
            live_database,
            archive_database,
        };
        copy.assert_sources_unchanged()?;
        Ok(copy)
    }

    /// Sandbox root containing all candidate artifacts.
    pub fn root(&self) -> &Path {
        self.sandbox.root()
    }

    /// Independent candidate live database.
    pub fn live_database(&self) -> &Path {
        &self.live_database
    }

    /// Independent candidate archive database when one was explicitly supplied.
    pub fn archive_database(&self) -> Option<&Path> {
        self.archive_database.as_deref()
    }

    /// Verify source bytes and metadata without constructing a Spirit store.
    pub fn assert_sources_unchanged(&self) -> Result<(), CandidateCopyError> {
        Self::assert_source_unchanged(&self.live_source)?;
        if let Some(archive) = &self.archive_source {
            Self::assert_source_unchanged(archive)?;
        }
        Ok(())
    }

    fn copy_source(
        source_path: PathBuf,
        candidate_path: PathBuf,
        role: &'static str,
    ) -> Result<SourceEvidence, CandidateCopyError> {
        let source = StoreFileFingerprint::capture(&source_path)?;
        fs::copy(&source_path, &candidate_path).map_err(CandidateCopyError::Io)?;
        let candidate = StoreFileFingerprint::capture(&candidate_path)?;
        if !source.content_and_mode_match(&candidate) {
            return Err(CandidateCopyError::CopyMismatch { role });
        }
        if source.file_identity() == candidate.file_identity() {
            return Err(CandidateCopyError::SharedFileIdentity { role });
        }
        Ok(SourceEvidence {
            path: source_path,
            fingerprint: source,
        })
    }

    fn assert_source_unchanged(source: &SourceEvidence) -> Result<(), CandidateCopyError> {
        if StoreFileFingerprint::capture(&source.path)? == source.fingerprint {
            Ok(())
        } else {
            Err(CandidateCopyError::SourceChanged(source.path.clone()))
        }
    }
}

/// Refusal while fingerprinting or preparing an isolated candidate copy.
#[derive(Debug, thiserror::Error)]
pub enum CandidateCopyError {
    /// Filesystem access failed.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// A source path was a symlink.
    #[error("candidate source must not be a symlink: {}", .0.display())]
    Symlink(PathBuf),

    /// A source path was not a regular file.
    #[error("candidate source must be a regular file: {}", .0.display())]
    NonRegular(PathBuf),

    /// A source did not name a `.sema` file.
    #[error("candidate source must have the .sema extension: {}", .0.display())]
    WrongExtension(PathBuf),

    /// Copied bytes or permission metadata differed from the source.
    #[error("{role} copy did not preserve bytes, length, and permission metadata")]
    CopyMismatch {
        /// Candidate role.
        role: &'static str,
    },

    /// The candidate reused the source filesystem object.
    #[error("{role} copy shares the source filesystem identity")]
    SharedFileIdentity {
        /// Candidate role.
        role: &'static str,
    },

    /// A source changed after its initial fingerprint.
    #[error("candidate source changed while the isolated copy was exercised: {}", .0.display())]
    SourceChanged(PathBuf),
}
