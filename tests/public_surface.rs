const LIB_RS: &str = include_str!("../src/lib.rs");

#[test]
fn crate_root_does_not_reexport_generated_plane_nouns() {
    assert!(
        LIB_RS.contains("pub mod schema"),
        "generated plane nouns stay publicly reachable through spirit::schema"
    );
    assert!(
        !LIB_RS
            .lines()
            .any(|line| line.trim_start().starts_with("pub use schema::")),
        "crate root must not flatten Signal/Nexus/SEMA generated nouns into spirit::*"
    );
}
