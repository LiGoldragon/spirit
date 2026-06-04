use std::{env, path::PathBuf};

use schema_rust_next::build::{GenerationDriver, GenerationPlan};

fn main() {
    SchemaBuild::from_environment().run();
}

struct SchemaBuild {
    crate_root: PathBuf,
}

impl SchemaBuild {
    fn from_environment() -> Self {
        Self {
            crate_root: PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir set")),
        }
    }

    fn run(&self) {
        println!("cargo:rerun-if-changed=schema/lib.schema");
        println!("cargo:rerun-if-changed=schema/lib.asschema");
        println!("cargo:rerun-if-changed=src/schema/lib.rs");

        let plan =
            GenerationPlan::component_runtime_compatibility(&self.crate_root, "spirit", "0.1.0");
        GenerationDriver::new(plan)
            .generate()
            .expect("generate spirit schema artifacts")
            .write_or_check("SPIRIT_UPDATE_SCHEMA_ARTIFACTS")
            .expect("checked-in spirit schema artifacts are fresh");
    }
}
