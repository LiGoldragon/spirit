use std::{env, fs, path::Path};

use spirit_next::{Input, SignalTransport};

fn main() {
    if let Err(error) = SpiritNextCli::from_environment().run() {
        eprintln!("spirit-next: {error}");
        std::process::exit(1);
    }
}

struct SpiritNextCli {
    arguments: Vec<String>,
}

impl SpiritNextCli {
    fn from_environment() -> Self {
        Self {
            arguments: env::args().skip(1).collect(),
        }
    }

    fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let argument = self.single_argument()?;
        let source = self.read_single_argument(argument)?;
        let input = source.parse::<Input>()?;
        let socket_path = env::var("SPIRIT_NEXT_SOCKET")
            .unwrap_or_else(|_| String::from("/tmp/spirit-next.sock"));
        let (_route, output) = SignalTransport::connect(socket_path)?.exchange(&input)?;
        println!("{output}");
        Ok(())
    }

    fn single_argument(&self) -> Result<&str, Box<dyn std::error::Error>> {
        match self.arguments.as_slice() {
            [argument] => Ok(argument),
            _ => Err("expected exactly one NOTA argument or path".into()),
        }
    }

    fn read_single_argument(&self, argument: &str) -> Result<String, Box<dyn std::error::Error>> {
        if argument.trim_start().starts_with('(') {
            Ok(argument.to_owned())
        } else if Path::new(argument).exists() {
            Ok(fs::read_to_string(argument)?)
        } else {
            Err("inline operation must be a parenthesized NOTA value".into())
        }
    }
}
