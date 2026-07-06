//! The spirit-side criome authorization option witness (Spirit `xhwa`).
//!
//! The `CriomeAuthorization` option decides what happens at the
//! authorize-and-ship seam (`Engine::drain_propagation_once`,
//! `MirrorShipper`):
//!
//!   (a) `Disabled` — the operative default. Spirit is fully local: heads
//!       advance freely, and the propagation drain is dormant — no
//!       authorization request, no mirror ship, even with an armed mirror
//!       target. Nothing propagates.
//!
//!   (b) `Enabled(authorizer)` — cluster authorization. The LOCAL commit
//!       stands (working inputs are never refused at ingress); only
//!       propagation waits on the cluster verdict. With no reachable criome
//!       behind the configured socket the drain decides `Unreachable` and
//!       holds every head back fail-closed: nothing ships, the suffix waits
//!       in the outbox.
//!
//! Falsification: if the dormant seam still propagated, the Disabled drain
//! would mark the outbox `ServerCommitted`; if the Enabled drain shipped on
//! an unreachable criome, the outbox would drain without a grant.

mod support;

use support::domain_fixtures;
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
use spirit::{ClusterAuthorizer, CriomeAuthorization, Engine, GateDecision, Store};
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
            domains: domain_fixtures::domains(&["Information/Documentation"]),
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
        assert_eq!(
            engine.criome_authorization(),
            &CriomeAuthorization::Disabled
        );

        // The head advances freely — spirit fully local.
        record(&mut engine, "a fully local head advance").await;
        assert!(
            engine
                .versioned_log_head()
                .expect("versioned head reads")
                .is_some(),
            "the working write advanced the local head"
        );

        // The authorize-and-ship seam is DORMANT: the propagation drain
        // completes with no gate decision and no ship, even though a live
        // mirror is armed and reachable.
        let decision = engine
            .drain_propagation_once()
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
fn enabled_authorization_holds_heads_back_when_criome_is_unreachable() {
    let runtime = runtime();
    let mirror_directory = tempfile::tempdir().expect("mirror temp dir");
    let component_directory = tempfile::tempdir().expect("component temp dir");

    runtime.block_on(async {
        let (_link, mirror_address) = running_mirror(&mirror_directory).await;
        let mut engine = armed_spirit_engine(&component_directory, "source.sema", mirror_address);

        // Enable cluster authorization against a socket no criome serves —
        // the enabled gate always has a socket; reachability is the drain's
        // problem, never ingress policy.
        let missing_socket = component_directory.path().join("no-criome.sock");
        engine.set_criome_authorization(CriomeAuthorization::Enabled(ClusterAuthorizer::new(
            &missing_socket,
        )));
        assert!(matches!(
            engine.criome_authorization(),
            CriomeAuthorization::Enabled(_)
        ));

        // The LOCAL commit stands: working inputs are admitted and the head
        // advances freely — only propagation waits on the cluster verdict.
        record(&mut engine, "a local head advance under an enabled gate").await;
        let local_head = engine
            .versioned_log_head()
            .expect("versioned head reads")
            .expect("the local commit advanced the head");

        // The drain asks the (absent) criome and decides Unreachable: the
        // head is held, nothing ships, the suffix waits in the outbox.
        let decision = engine
            .drain_propagation_once()
            .await
            .expect("the drain completes without machinery fault")
            .expect("a head exists to authorize");
        assert!(
            matches!(decision, GateDecision::Unreachable),
            "an unreachable criome holds the head back, got {decision:?}"
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

        // Reads stay served, and the local head is untouched by the refusal.
        let version = engine.handle_async(Input::Version).await.into_root();
        assert!(
            matches!(version, Output::VersionReported(_)),
            "reads stay admitted while authorization is enabled, got {version:?}"
        );
        assert_eq!(
            engine
                .versioned_log_head()
                .expect("versioned head reads")
                .expect("the local head remains"),
            local_head,
            "holding propagation back never rolls the local commit back"
        );
    });
}
