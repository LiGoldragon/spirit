//! The spirit-side criome authorization option witness (Spirit `xhwa`).
//!
//! The 1-of-1 LOCAL criome authorization path is DELETED; the
//! authorize-and-ship seam (`CriomeGate::authorize_head`, `MirrorShipper`)
//! stays in place, dormant, for the coming criome-cluster authorization flow.
//! The `CriomeAuthorization` option decides what happens at the seam today:
//!
//!   (a) `Disabled` — the operative default. Spirit is fully local: heads
//!       advance freely, and `gate_and_ship_head` is a no-op — no
//!       authorization request, no mirror ship, even with an armed mirror
//!       target. Nothing propagates.
//!
//!   (b) `Enabled` — fail-closed. No criome-cluster authorizer exists yet, so
//!       a head-advancing working input is REFUSED before any pipeline write:
//!       the head does not advance, the store does not change. Reads stay
//!       admitted. A pre-existing head is held back `Unconfigured` — it never
//!       ships.
//!
//! Falsification: if the dormant seam still propagated, the Disabled drain
//! would mark the outbox `ServerCommitted`; if the Enabled refusal leaked a
//! write, the head digest or record count would move.

use std::net::SocketAddr;

use mirror::{Engine as MirrorEngine, Service, ServiceLink};
use sema_engine::Durability;
use spirit::schema::meta_signal::{
    ArchiveDatabaseTarget, ConfigureRequest, MirrorAddress, MirrorAddressText, MirrorTarget,
    Output as MetaOutput,
};
use spirit::schema::sema::RecordFamily;
use spirit::schema::signal::{
    Certainty, Description, Domains, Entry, Importance, Input, Justification, Kind, Magnitude,
    Output, Privacy, QuoteText, Reasoning, RecordRequest, Referent, Referents, Testimony,
    VerbatimQuote,
};
use spirit::{CriomeAuthorization, Engine, GateDecision, Store};
use tempfile::TempDir;
use triad_runtime::kameo::actor::Spawn;

const STORE_NAME: &str = RecordFamily::STORE_NAME;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn record_request(description: &str) -> RecordRequest {
    RecordRequest {
        entry: Entry {
            domains: Domains::from_strings(vec![String::from("Information/Documentation")]),
            kind: Kind::Decision,
            description: Description::new(description),
            certainty: Certainty::new(Magnitude::High),
            importance: Importance::new(Magnitude::Medium),
            privacy: Privacy::new(Magnitude::Zero),
            referents: Referents::new(vec![Referent::new("spirit")]),
        },
        justification: Justification {
            testimony: Testimony::new(vec![VerbatimQuote::new(QuoteText::new(description), None)]),
            reasoning: Reasoning::new(description),
        },
    }
}

async fn record(engine: &mut Engine, description: &str) {
    let output = engine
        .handle_async(Input::record(record_request(description)))
        .await
        .into_root();
    assert!(
        matches!(output, Output::RecordAccepted(_)),
        "record accepted, got {output:?}"
    );
}

fn mirror_target(address: SocketAddr) -> MirrorTarget {
    MirrorTarget::Address(MirrorAddress::new(MirrorAddressText::new(
        address.to_string(),
    )))
}

/// Stand up an in-process mirror daemon (real engine, real store, loopback
/// TCP) and register the spirit store on its meta surface — the fan-out
/// target the dormant seam must NOT reach.
async fn running_mirror(directory: &TempDir) -> (ServiceLink, SocketAddr) {
    let store =
        mirror::Store::open(&directory.path().join("mirror.sema")).expect("mirror store opens");
    let service = Service::spawn(Service::new(
        MirrorEngine::new(store),
        "127.0.0.1:0".parse().expect("loopback address"),
    ));
    service.wait_for_startup().await;
    let link = ServiceLink::new(service);
    let address = link
        .tcp_bound_address()
        .await
        .expect("query bound address")
        .expect("the tailnet ingress is bound");
    let registered = link
        .meta(meta_signal_mirror::Input::RegisterStore(
            meta_signal_mirror::StoreRegistration {
                store: meta_signal_mirror::StoreName::new(STORE_NAME.to_owned()),
                addressing: meta_signal_mirror::ContentAddressing::Opaque,
            },
        ))
        .await
        .expect("meta register");
    assert!(matches!(
        registered,
        meta_signal_mirror::Output::StoreRegistered(_)
    ));
    (link, address)
}

/// Open a fresh spirit engine with its mirror shipper armed at the in-process
/// mirror, so a leaked ship would be observable.
fn armed_spirit_engine(directory: &TempDir, name: &str, mirror_address: SocketAddr) -> Engine {
    let store = Store::open(directory.path().join(name)).expect("open spirit store");
    let mut engine = Engine::new(store);
    engine.start().expect("engine starts");
    let configured = engine.configure(ConfigureRequest::new(
        ArchiveDatabaseTarget::Default,
        Some(mirror_target(mirror_address)),
        None,
        None,
    ));
    assert!(
        matches!(configured, MetaOutput::Configured(_)),
        "configure accepted, got {configured:?}"
    );
    assert!(engine.mirror_shipping_armed(), "the shipper is armed");
    engine
}

#[test]
fn disabled_default_advances_heads_freely_and_keeps_the_ship_seam_dormant() {
    let runtime = runtime();
    let mirror_directory = tempfile::tempdir().expect("mirror temp dir");
    let component_directory = tempfile::tempdir().expect("component temp dir");

    runtime.block_on(async {
        let (_link, mirror_address) = running_mirror(&mirror_directory).await;
        let mut engine = armed_spirit_engine(&component_directory, "source.sema", mirror_address);

        // Disabled is the operative default: no owner action selects it.
        assert_eq!(engine.criome_authorization(), CriomeAuthorization::Disabled);

        // The head advances freely — spirit fully local.
        record(&mut engine, "a fully local head advance").await;
        assert!(
            engine
                .versioned_log_head()
                .expect("versioned head reads")
                .is_some(),
            "the working write advanced the local head"
        );

        // The authorize-and-ship seam is DORMANT: the daemon's post-commit
        // hook completes with no gate decision and no ship, even though a
        // live mirror is armed and reachable.
        let decision = engine
            .gate_and_ship_head()
            .await
            .expect("the dormant seam completes without machinery fault");
        assert!(
            decision.is_none(),
            "disabled authorization keeps the seam dormant, got {decision:?}"
        );

        let handle = engine.store().engine_handle();
        assert_eq!(
            handle.store_durability().expect("durability reads"),
            Durability::QueuedForMirror,
            "nothing propagates while criome authorization is disabled"
        );
        assert!(
            !handle.unshipped_outbox().expect("outbox reads").is_empty(),
            "the local history stays unshipped in the outbox"
        );
    });
}

#[test]
fn enabled_authorization_refuses_head_advances_fail_closed() {
    let runtime = runtime();
    let mirror_directory = tempfile::tempdir().expect("mirror temp dir");
    let component_directory = tempfile::tempdir().expect("component temp dir");

    runtime.block_on(async {
        let (_link, mirror_address) = running_mirror(&mirror_directory).await;
        let mut engine = armed_spirit_engine(&component_directory, "source.sema", mirror_address);

        // A head committed while authorization was disabled — the pre-existing
        // local history the enabled gate must hold back.
        record(&mut engine, "a head committed before enabling").await;
        let settled_head = engine
            .versioned_log_head()
            .expect("versioned head reads")
            .expect("a committed head exists");

        engine.set_criome_authorization(CriomeAuthorization::Enabled);
        assert_eq!(engine.criome_authorization(), CriomeAuthorization::Enabled);

        // A head-advancing input is REFUSED before any pipeline write: no
        // cluster authorizer exists yet, so the head must not advance.
        let refused = engine
            .handle_async(Input::record(record_request(
                "a head advance the enabled gate must refuse",
            )))
            .await
            .into_root();
        assert!(
            matches!(refused, Output::Error(_)),
            "an enabled gate refuses the head advance, got {refused:?}"
        );
        assert_eq!(engine.record_count(), 1, "the refused write never landed");
        assert_eq!(
            engine
                .versioned_log_head()
                .expect("versioned head reads")
                .expect("the settled head remains"),
            settled_head,
            "the head did not advance under refusal"
        );

        // Reads carry no head advance and stay admitted.
        let version = engine.handle_async(Input::Version).await.into_root();
        assert!(
            matches!(version, Output::VersionReported(_)),
            "reads stay admitted while authorization is enabled, got {version:?}"
        );

        // The pre-existing head is held back fail-closed: the dormant seam
        // answers Unconfigured (no cluster authorizer), and nothing ships.
        let decision = engine
            .gate_and_ship_head()
            .await
            .expect("the gate completes without machinery fault")
            .expect("a head exists to authorize");
        assert!(
            matches!(decision, GateDecision::Unconfigured),
            "no cluster authorizer holds the head back, got {decision:?}"
        );
        let handle = engine.store().engine_handle();
        assert_eq!(
            handle.store_durability().expect("durability reads"),
            Durability::QueuedForMirror,
            "an unauthorized head must not ship"
        );
        assert!(
            !handle.unshipped_outbox().expect("outbox reads").is_empty(),
            "the held-back history stays unshipped in the outbox"
        );
    });
}
