use std::env;

use spirit_next::{Configuration, run_daemon};

fn main() {
    if let Err(error) = run() {
        eprintln!("spirit-next-daemon: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args().skip(1);
    let argument = arguments
        .next()
        .ok_or("expected exactly one NOTA configuration argument or path")?;
    if arguments.next().is_some() {
        return Err("expected exactly one NOTA configuration argument or path".into());
    }
    let configuration = Configuration::from_single_argument(&argument)?;
    run_daemon(configuration)?;
    Ok(())
}
