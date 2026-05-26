use std::{env, fs, path::PathBuf};

use schema_next::{SchemaEngine, SchemaIdentity};
use schema_rust_next::RustEmitter;

fn main() {
    println!("cargo:rerun-if-changed=schema/spirit.schema");

    let source = fs::read_to_string("schema/spirit.schema").expect("read schema/spirit.schema");
    let asschema = SchemaEngine::default()
        .lower_source(&source, SchemaIdentity::new("spirit_next", "0.1.0"))
        .expect("lower spirit-next schema");
    let generated = RustEmitter.emit_file(&asschema);

    let output_directory = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR set"));
    fs::write(
        output_directory.join("spirit_next_generated.rs"),
        generated.code.as_str(),
    )
    .expect("write generated Spirit interface");
}
