use std::{env, fs, path::PathBuf};

use schema_next::{MacroContext, SchemaEngine, SchemaPackage};
use schema_rust_next::{GeneratedFile, RustEmitter};

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
        println!("cargo:rerun-if-changed=src/schema/lib.rs");

        let generated = self.generated_schema_file();
        self.assert_generated_schema_path(&generated);
        self.assert_checked_in_schema_is_fresh(&generated);
    }

    fn generated_schema_file(&self) -> GeneratedFile {
        let mut context = MacroContext::default();
        let package = SchemaPackage::new(&self.crate_root, "spirit-next", "0.1.0");
        let source = package.load_lib().expect("read schema/lib.schema");
        let asschema = SchemaEngine::default()
            .lower_source_with_context(source.source(), source.identity().clone(), &mut context)
            .expect("lower spirit-next schema");
        self.assert_schema_macros_were_used(&context);
        RustEmitter::default().emit_file(&asschema)
    }

    fn assert_schema_macros_were_used(&self, context: &MacroContext) {
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
    }

    fn assert_generated_schema_path(&self, generated: &GeneratedFile) {
        if generated.path.as_str() != "src/schema/lib.rs" {
            panic!(
                "spirit-next schema must emit src/schema/lib.rs, found {}",
                generated.path
            );
        }
    }

    fn assert_checked_in_schema_is_fresh(&self, generated: &GeneratedFile) {
        let checked_in = CheckedInSchemaSource::new(&self.crate_root, generated);
        let actual = fs::read_to_string(checked_in.path()).unwrap_or_else(|error| {
            panic!(
                "checked-in generated schema source is missing at {}: {error}",
                checked_in.path().display()
            )
        });
        let expected = checked_in.expected_source();
        if actual != expected {
            panic!(
                "checked-in generated schema source is stale at {}; regenerate it from schema/lib.schema",
                checked_in.path().display()
            );
        }
    }
}

struct CheckedInSchemaSource<'schema> {
    crate_root: &'schema PathBuf,
    generated: &'schema GeneratedFile,
}

impl<'schema> CheckedInSchemaSource<'schema> {
    fn new(crate_root: &'schema PathBuf, generated: &'schema GeneratedFile) -> Self {
        Self {
            crate_root,
            generated,
        }
    }

    fn path(&self) -> PathBuf {
        self.crate_root.join(&self.generated.path)
    }

    fn expected_source(&self) -> String {
        self.generated.code.as_str().to_owned()
    }
}
