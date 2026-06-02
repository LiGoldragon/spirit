use spirit_next::DaemonCommand;

fn main() {
    if let Err(error) = DaemonCommand::from_environment().run() {
        eprintln!("spirit-next-daemon: {error}");
        std::process::exit(1);
    }
}
