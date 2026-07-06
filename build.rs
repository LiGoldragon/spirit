use std::{env, path::PathBuf};

use schema_rust::{
    MetaListenerTier, NexusDaemonShape, SocketModeBits, WorkingListenerTier,
    build::{DependencySchema, GenerationDriver, GenerationPlan, ModuleEmission},
};

/// The owner-only meta socket file mode: readable and writable by the owner
/// uid only (`rw-------`). triad-runtime has no peer-credential check, so the
/// meta surface's owner-only authority rests on this filesystem mode.
const OWNER_ONLY_SOCKET_MODE: u32 = 0o600;

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
        println!("cargo:rerun-if-changed=schema/nexus.schema");
        println!("cargo:rerun-if-changed=src/schema/nexus.rs");
        println!("cargo:rerun-if-changed=schema/sema.schema");
        println!("cargo:rerun-if-changed=src/schema/sema.rs");
        println!("cargo:rerun-if-changed=src/schema/daemon.rs");
        println!("cargo:rerun-if-env-changed=DEP_SIGNAL_DOMAIN_SCHEMA_DIR");
        println!("cargo:rerun-if-env-changed=DEP_SIGNAL_SPIRIT_SCHEMA_DIR");
        println!("cargo:rerun-if-env-changed=DEP_META_SIGNAL_SPIRIT_SCHEMA_DIR");

        let signal_domain =
            DependencySchema::from_cargo_metadata("signal-domain", "signal-domain", "0.1.0")
                .expect("read signal-domain schema metadata");
        let Some(signal_spirit) =
            DependencySchema::from_cargo_metadata("signal-spirit", "signal-spirit", "0.12.0")
                .expect("read signal-spirit schema metadata")
        else {
            return;
        };

        let Some(meta_signal_spirit) = DependencySchema::from_cargo_metadata(
            "meta-signal-spirit",
            "meta-signal-spirit",
            "0.7.0",
        )
        .expect("read meta-signal-spirit schema metadata") else {
            return;
        };

        let plan = GenerationPlan::new(&self.crate_root, "spirit", "0.5.0")
            .with_optional_dependency_schema(signal_domain)
            .with_dependency_schema(signal_spirit)
            .with_dependency_schema(meta_signal_spirit)
            .with_module(ModuleEmission::nexus_runtime())
            .with_module(ModuleEmission::sema_runtime())
            .with_module(ModuleEmission::daemon_module("nexus", self.daemon_shape()));
        GenerationDriver::new(plan)
            .generate()
            .expect("generate spirit schema artifacts")
            .write_or_check("SPIRIT_UPDATE_SCHEMA_ARTIFACTS")
            .expect("checked-in spirit schema artifacts are fresh");
    }

    /// Spirit's daemon shape: the `spirit-daemon` process, a working listener
    /// whose wire contract is imported from `signal-spirit`, and an owner-only
    /// meta listener whose wire contract is imported from `meta-signal-spirit`
    /// at mode `0o600`.
    fn daemon_shape(&self) -> NexusDaemonShape {
        NexusDaemonShape::new(
            "spirit-daemon",
            WorkingListenerTier::dependency("signal_spirit::schema::signal"),
        )
        .with_working_streams()
        .with_meta_tier(MetaListenerTier::new(SocketModeBits::new(
            OWNER_ONLY_SOCKET_MODE,
        )))
    }
}
