use std::{env, fs, path::Path};

use spirit_next::{Input, exchange};

fn main() {
    if let Err(error) = run() {
        eprintln!("spirit-next: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args().skip(1);
    let argument = arguments
        .next()
        .ok_or("expected exactly one NOTA argument or path")?;
    if arguments.next().is_some() {
        return Err("expected exactly one NOTA argument or path".into());
    }

    let source = read_single_argument(&argument)?;
    let input = source.parse::<Input>()?;
    let socket_path =
        env::var("SPIRIT_NEXT_SOCKET").unwrap_or_else(|_| String::from("/tmp/spirit-next.sock"));
    let (_route, output) = exchange(socket_path, &input)?;
    println!("{output}");
    Ok(())
}

fn read_single_argument(argument: &str) -> Result<String, Box<dyn std::error::Error>> {
    if argument.trim_start().starts_with('(') {
        Ok(argument.to_owned())
    } else if Path::new(argument).exists() {
        Ok(fs::read_to_string(argument)?)
    } else {
        Err("inline operation must be a parenthesized NOTA value".into())
    }
}
