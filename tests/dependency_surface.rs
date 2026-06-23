use std::{fs, path::PathBuf, process::Command};

struct WorkspaceManifest {
    path: PathBuf,
}

impl WorkspaceManifest {
    fn from_environment() -> Self {
        Self {
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        }
    }

    fn cargo_tree(&self, arguments: &[&str]) -> String {
        let output = Command::new(env!("CARGO"))
            .arg("tree")
            .args(arguments)
            .current_dir(&self.path)
            .output()
            .expect("cargo tree runs");
        assert!(
            output.status.success(),
            "cargo tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("cargo tree stdout is UTF-8")
    }

    fn cargo_toml(&self) -> String {
        fs::read_to_string(self.path.join("Cargo.toml")).expect("Cargo.toml is readable")
    }
}

#[test]
fn binary_only_surface_has_no_nota_runtime_dependency() {
    let manifest = WorkspaceManifest::from_environment();
    let tree = manifest.cargo_tree(&["--edges", "normal", "--no-default-features"]);

    assert!(
        !tree.contains("nota") && !tree.contains("nota"),
        "binary-only runtime dependency tree must not contain nota:\n{tree}"
    );
}

#[test]
fn spirit_has_no_direct_redb_dependency_or_redb_two_runtime_tree() {
    let manifest = WorkspaceManifest::from_environment();
    let cargo = manifest.cargo_toml();
    let tree = manifest.cargo_tree(&["--edges", "normal", "--no-default-features"]);

    assert!(
        !cargo.contains("\nredb ="),
        "Spirit must not depend on redb directly; storage goes through sema-engine"
    );
    assert!(
        !tree.contains("redb v2."),
        "normal runtime tree must not contain redb 2.x:\n{tree}"
    );
}

#[test]
fn text_client_surface_has_nota_runtime_dependency() {
    let manifest = WorkspaceManifest::from_environment();
    let tree = manifest.cargo_tree(&["--edges", "normal", "--features", "nota-text"]);

    assert!(
        tree.contains("nota"),
        "nota-text runtime dependency tree must contain nota:\n{tree}"
    );
}
