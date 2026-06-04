use spirit_next::{Configuration, DaemonCommand, DaemonCommandError};
use triad_runtime::ArgumentError;

struct TemporaryConfiguration {
    directory: tempfile::TempDir,
}

impl TemporaryConfiguration {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().expect("create tempdir"),
        }
    }

    fn configuration_path(&self) -> std::path::PathBuf {
        self.directory.path().join("configuration.rkyv")
    }

    fn socket_path(&self) -> std::path::PathBuf {
        self.directory.path().join("spirit.sock")
    }

    fn database_path(&self) -> std::path::PathBuf {
        self.directory.path().join("spirit.sema")
    }

    fn write_configuration(&self) -> Configuration {
        let configuration = Configuration::new(self.socket_path(), self.database_path());
        configuration
            .write_binary_file(self.configuration_path())
            .expect("write binary configuration");
        configuration
    }
}

#[test]
fn daemon_command_reads_the_single_binary_configuration_argument() {
    let temporary_configuration = TemporaryConfiguration::new();
    let configuration = temporary_configuration.write_configuration();
    let command = DaemonCommand::from_arguments([temporary_configuration
        .configuration_path()
        .to_string_lossy()
        .into_owned()]);

    let read_configuration = command.configuration().expect("read configuration");

    assert_eq!(read_configuration, configuration);
}

#[test]
fn daemon_command_rejects_missing_or_extra_arguments() {
    let missing = DaemonCommand::from_arguments(std::iter::empty::<String>());
    let extra = DaemonCommand::from_arguments(["one", "two"]);

    assert!(matches!(
        missing.configuration(),
        Err(DaemonCommandError::Argument(ArgumentError::ArgumentCount {
            count: 0
        }))
    ));
    assert!(matches!(
        extra.configuration(),
        Err(DaemonCommandError::Argument(ArgumentError::ArgumentCount {
            count: 2
        }))
    ));
}
