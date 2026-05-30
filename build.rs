use std::{
    env, fs,
    path::{Path, PathBuf},
};

use schema_next::{AsschemaArtifact, SchemaEngine, SchemaPackage};
use schema_rust_next::{GeneratedFile, RustEmissionOptions, RustEmitter};

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
            output_directory: PathBuf::from(env::var_os("OUT_DIR").expect("out dir set")),
        }
    }

    fn run(&self) {
        println!("cargo:rerun-if-changed=schema/lib.schema");
        println!("cargo:rerun-if-changed=schema/lib.asschema");
        println!("cargo:rerun-if-changed=src/schema/lib.rs");

        let generated = self.generated_schema_file();
        self.assert_generated_schema_path(&generated);
        self.assert_checked_in_schema_is_fresh(&generated);
    }

    fn generated_schema_file(&self) -> GeneratedFile {
        let package = SchemaPackage::new(&self.crate_root, "spirit-next", "0.1.0");
        let source = package.load_lib().expect("read schema/lib.schema");
        let asschema = SchemaEngine::default()
            .lower_source(source.source(), source.identity().clone())
            .expect("lower spirit-next schema");
        let artifact = AsschemaArtifact::new(asschema);
        let artifact_files = GeneratedAsschemaArtifactFiles::new(&self.output_directory);
        artifact
            .write_nota_file(artifact_files.nota_path())
            .expect("write generated asschema NOTA artifact");
        artifact
            .write_binary_file(artifact_files.binary_path())
            .expect("write generated asschema rkyv artifact");

        let checked_in_artifact = CheckedInAsschemaArtifact::new(&self.crate_root);
        checked_in_artifact.assert_matches_generated_artifact(&artifact_files);

        RustEmitter::new(RustEmissionOptions::feature_gated_nota("nota-text"))
            .emit_file_from_nota_path(checked_in_artifact.path())
            .expect("emit Rust from checked-in asschema NOTA artifact")
            .assert_matches_binary_artifact(&artifact_files)
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

struct CheckedInAsschemaArtifact {
    path: PathBuf,
}

impl CheckedInAsschemaArtifact {
    fn new(crate_root: &Path) -> Self {
        Self {
            path: crate_root.join("schema").join("lib.asschema"),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn assert_matches_generated_artifact(&self, artifact_files: &GeneratedAsschemaArtifactFiles) {
        let checked_in = fs::read_to_string(self.path()).unwrap_or_else(|error| {
            panic!(
                "checked-in assembled schema artifact is missing at {}: {error}",
                self.path().display()
            )
        });
        let generated = fs::read_to_string(artifact_files.nota_path())
            .expect("read generated asschema artifact");
        if checked_in != generated {
            panic!(
                "checked-in assembled schema artifact is stale at {}; regenerate it from schema/lib.schema",
                self.path().display()
            );
        }
    }
}

struct GeneratedAsschemaArtifactFiles {
    nota_path: PathBuf,
    binary_path: PathBuf,
}

impl GeneratedAsschemaArtifactFiles {
    fn new(output_directory: &Path) -> Self {
        Self {
            nota_path: output_directory.join("lib.asschema"),
            binary_path: output_directory.join("lib.asschema.rkyv"),
        }
    }

    fn nota_path(&self) -> &Path {
        &self.nota_path
    }

    fn binary_path(&self) -> &Path {
        &self.binary_path
    }
}

trait GeneratedFileArtifactWitness {
    fn assert_matches_binary_artifact(
        self,
        artifact_files: &GeneratedAsschemaArtifactFiles,
    ) -> Self;
}

impl GeneratedFileArtifactWitness for GeneratedFile {
    fn assert_matches_binary_artifact(
        self,
        artifact_files: &GeneratedAsschemaArtifactFiles,
    ) -> Self {
        let from_binary = RustEmitter::new(RustEmissionOptions::feature_gated_nota("nota-text"))
            .emit_file_from_binary_path(artifact_files.binary_path())
            .expect("emit Rust from generated asschema rkyv artifact");
        if self != from_binary {
            panic!(
                "generated Rust differs between asschema NOTA artifact {} and rkyv artifact {}",
                artifact_files.nota_path().display(),
                artifact_files.binary_path().display()
            );
        }
        self
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
