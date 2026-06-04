use spirit::DaemonCommand;

fn main() {
    if let Err(error) = DaemonCommand::from_environment().run() {
        eprintln!("spirit-daemon: {error}");
        std::process::exit(1);
    }
}
