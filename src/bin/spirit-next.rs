use std::{env, fs, path::Path};

use spirit_next::{Input, SignalTransport};

#[cfg(feature = "testing-trace")]
use spirit_next::TraceSocketListener;
#[cfg(feature = "testing-trace")]
use std::time::Duration;

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
        #[cfg(feature = "testing-trace")]
        let trace_output = TraceOutput::from_environment()?;
        let (_route, output) = SignalTransport::connect(socket_path)?.exchange(&input)?;
        println!("{output}");
        #[cfg(feature = "testing-trace")]
        trace_output.print_events()?;
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

#[cfg(feature = "testing-trace")]
struct TraceOutput {
    listener: Option<TraceSocketListener>,
}

#[cfg(feature = "testing-trace")]
impl TraceOutput {
    fn from_environment() -> Result<Self, Box<dyn std::error::Error>> {
        let listener = match env::var("SPIRIT_NEXT_TRACE_SOCKET") {
            Ok(path) => Some(TraceSocketListener::bind(path)?),
            Err(env::VarError::NotPresent) => None,
            Err(error) => return Err(Box::new(error)),
        };
        Ok(Self { listener })
    }

    fn print_events(&self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(listener) = &self.listener {
            for event in listener.collect_for(Duration::from_millis(200))? {
                println!("{event}");
            }
        }
        Ok(())
    }
}
