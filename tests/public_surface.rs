const LIB_RS: &str = include_str!("../src/lib.rs");

#[test]
fn crate_root_does_not_reexport_generated_plane_nouns() {
    assert!(
        LIB_RS.contains("pub mod schema"),
        "generated plane nouns stay publicly reachable through spirit::schema"
    );
    // The generated contract/plane modules carry the wire/runtime nouns and
    // must stay reached through `spirit::schema::{signal,meta_signal,nexus,sema}`.
    // The emitted daemon entry-point module (`schema::daemon`) is the daemon's
    // public API — `DaemonCommand` / `DaemonEntry` / `DaemonError` /
    // `ComponentDaemon` — and is re-exported at the crate root deliberately, so
    // it is exempt.
    let flattens_plane_noun = |line: &str| {
        let line = line.trim_start();
        line.starts_with("pub use schema::signal::")
            || line.starts_with("pub use schema::nexus::")
            || line.starts_with("pub use schema::sema::")
            || line.starts_with("pub use schema::meta_signal::")
    };
    assert!(
        !LIB_RS.lines().any(flattens_plane_noun),
        "crate root must not flatten Signal/Nexus/SEMA generated nouns into spirit::*"
    );
}
