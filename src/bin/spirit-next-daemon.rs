use std::env;

use spirit_next::{Configuration, Daemon};

fn main() {
    if let Err(error) = SpiritNextDaemonCli::from_environment().run() {
        eprintln!("spirit-next-daemon: {error}");
        std::process::exit(1);
    }
}

struct SpiritNextDaemonCli {
    arguments: Vec<String>,
}

impl SpiritNextDaemonCli {
    fn from_environment() -> Self {
        Self {
            arguments: env::args().skip(1).collect(),
        }
    }

    fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let configuration = Configuration::from_single_argument(self.single_argument()?)?;
        Daemon::new(configuration).run()?;
        Ok(())
    }

    fn single_argument(&self) -> Result<&str, Box<dyn std::error::Error>> {
        match self.arguments.as_slice() {
            [argument] => Ok(argument),
            _ => Err("expected exactly one NOTA configuration argument or path".into()),
        }
    }
}
