fn main() {
    if let Err(error) = spirit::render::RenderCli::from_environment().run() {
        eprintln!("spirit-render: {error}");
        std::process::exit(1);
    }
}
