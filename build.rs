use std::{env, fs, path::PathBuf};

use schema_next::{MacroContext, SchemaEngine, SchemaPackage};
use schema_rust_next::RustEmitter;

fn main() {
    SchemaBuild::from_environment().run();
}

struct SchemaBuild {
    crate_root: PathBuf,
    output_directory: PathBuf,
}

impl SchemaBuild {
    fn from_environment() -> Self {
        Self {
            crate_root: PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir set")),
            output_directory: PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR set")),
        }
    }

    fn run(&self) {
        println!("cargo:rerun-if-changed=schema/lib.schema");

        let mut context = MacroContext::default();
        let package = SchemaPackage::new(&self.crate_root, "spirit-next", "0.1.0");
        let source = package.load_lib().expect("read schema/lib.schema");
        let asschema = SchemaEngine::default()
            .lower_source_with_context(source.source(), source.identity().clone(), &mut context)
            .expect("lower spirit-next schema");
        if !context
            .macros_applied()
            .windows(2)
            .any(|pair| pair == ["SchemaStructDefinition", "SchemaStructFields"])
            || !context
                .macros_applied()
                .windows(2)
                .any(|pair| pair == ["SchemaEnumDefinition", "SchemaEnumVariants"])
        {
            panic!(
                "spirit-next schema generation must use schema-next registry macros for type bodies"
            );
        }
        let generated = RustEmitter::default().emit_file(&asschema);
        let output_path = self.output_directory.join(&generated.path);
        let parent = output_path.parent().expect("generated path has parent");
        fs::create_dir_all(parent).expect("create generated schema module directory");
        fs::write(output_path, generated.code.as_str()).expect("write generated Spirit interface");
    }
}
