use spirit::{DaemonEntry, SpiritDaemon};

fn main() -> std::process::ExitCode {
    SpiritDaemon::run_to_exit_code()
}
