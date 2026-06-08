use std::{env, path::PathBuf};

use schema_rust_next::{
    MetaListenerTier, NexusDaemonShape, SocketModeBits, WorkingListenerTier,
    build::{GenerationDriver, GenerationPlan, ModuleEmission},
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
        println!("cargo:rerun-if-changed=schema/signal.schema");
        println!("cargo:rerun-if-changed=src/schema/signal.rs");
        println!("cargo:rerun-if-changed=schema/nexus.schema");
        println!("cargo:rerun-if-changed=src/schema/nexus.rs");
        println!("cargo:rerun-if-changed=schema/sema.schema");
        println!("cargo:rerun-if-changed=src/schema/sema.rs");
        println!("cargo:rerun-if-changed=schema/meta-signal.schema");
        println!("cargo:rerun-if-changed=src/schema/meta_signal.rs");
        println!("cargo:rerun-if-changed=src/schema/daemon.rs");

        let plan = GenerationPlan::new(&self.crate_root, "spirit", "0.3.0")
            .with_module(ModuleEmission::signal_runtime_module("signal"))
            .with_module(ModuleEmission::nexus_runtime())
            .with_module(ModuleEmission::sema_runtime())
            .with_module(ModuleEmission::wire_contract_module("meta-signal"))
            .with_module(ModuleEmission::daemon_module("signal", self.daemon_shape()));
        GenerationDriver::new(plan)
            .generate()
            .expect("generate spirit schema artifacts")
            .write_or_check("SPIRIT_UPDATE_SCHEMA_ARTIFACTS")
            .expect("checked-in spirit schema artifacts are fresh");
    }

    /// Spirit's daemon shape: the `spirit-daemon` process, a working signal
    /// listener (`schema/signal.schema`), and an owner-only meta listener
    /// (`schema/meta-signal.schema`) at mode `0o600`. The stream wiring is
    /// derived from the signal schema's `IntentEventStream` declaration.
    fn daemon_shape(&self) -> NexusDaemonShape {
        NexusDaemonShape::new("spirit-daemon", WorkingListenerTier::new("signal")).with_meta_tier(
            MetaListenerTier::new(SocketModeBits::new(OWNER_ONLY_SOCKET_MODE)),
        )
    }
}
