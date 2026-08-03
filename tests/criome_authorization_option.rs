//! The spirit-side criome authorization option witness (Spirit `xhwa`,
//! corrected to the everywhere-gate).
//!
//! The `CriomeAuthorization` option decides whether acceptance itself is
//! cluster-gated:
//!
//!   (a) `Disabled` — the operative default. Spirit is fully local: heads
//!       advance freely, and the ship seam is dormant — no authorization
//!       request, no mirror ship, even with an armed mirror target. Nothing
//!       propagates.
//!
//!   (b) `Enabled(authorizer)` — the everywhere-gate. A head-advancing
//!       working operation is STAGED and accepted only on the cluster
//!       grant. With no reachable criome behind the configured socket the
//!       verdict is `Unreachable` and the OPERATION is refused to the
//!       caller (`AdvanceRefused`): the head does not advance, not even
//!       locally, and nothing exists to propagate. Fail-closed. Reads are
//!       unaffected.
//!
//! Falsification: if the dormant seam still propagated, the Disabled drain
//! would mark the outbox `ServerCommitted`; if the Enabled intake accepted
//! on an unreachable criome, the head would advance without a grant.

mod support;

use std::net::SocketAddr;
use support::domain_fixtures;

use mirror::{Engine as MirrorEngine, Service, ServiceLink};
use sema_engine::Durability;
use spirit::schema::meta_signal::{
    ArchiveDatabaseTarget, ConfigureRequest, MirrorAddress, MirrorAddressText, MirrorTarget,
    Output as MetaOutput,
};
use spirit::schema::signal::{
    Description, Domains, Entry, Importance, Input, Justification, Kind, Magnitude, Output,
    QuoteText, Reasoning, RecordRequest, Testimony, VerbatimQuote,
};
use spirit::{ClusterAuthorizer, CriomeAuthorization, Engine, SPIRIT_STORE_NAME, Store};
use tempfile::TempDir;
use triad_runtime::kameo::actor::Spawn;

const STORE_NAME: &str = SPIRIT_STORE_NAME;

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
            importance: Importance::new(Magnitude::Medium),
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

/// THE CORRECTED OUTCOME: an enabled gate with an unreachable criome
/// REFUSES the operation to the caller — the head does NOT advance, not
/// even locally, and there is nothing to ship. Previously this leg asserted
/// "the local commit stands, only propagation waits"; that premise is
/// overridden by the everywhere-gate.
#[test]
fn enabled_authorization_refuses_head_advances_when_criome_is_unreachable() {
    let runtime = runtime();
    let mirror_directory = tempfile::tempdir().expect("mirror temp dir");
    let component_directory = tempfile::tempdir().expect("component temp dir");

    runtime.block_on(async {
        let (_link, mirror_address) = running_mirror(&mirror_directory).await;
        let mut engine = armed_spirit_engine(&component_directory, "source.sema", mirror_address);

        // Enable cluster authorization against a socket no criome serves —
        // the enabled gate always has a socket; an unreachable criome is a
        // typed refusal to the caller, never a default-open branch.
        let missing_socket = component_directory.path().join("no-criome.sock");
        engine.set_criome_authorization(CriomeAuthorization::Enabled(
            ClusterAuthorizer::new(&missing_socket)
                .with_session_deadline(std::time::Duration::from_secs(1)),
        ));
        assert!(matches!(
            engine.criome_authorization(),
            CriomeAuthorization::Enabled(_)
        ));

        // The staged intake parks the operation, the round decides
        // Unreachable, and the OPERATION is refused: nothing recorded.
        let staged = engine
            .stage_working_input(Input::record(record_request(
                "an operation refused by the everywhere-gate",
            )))
            .await;
        let spirit::StagedIntake::Parked(mut advance) = staged else {
            panic!("a head advance under an enabled gate parks, got {staged:?}");
        };
        advance.resolve().await;
        let reply = engine.conclude_staged_advance(advance).await;
        assert!(
            matches!(&reply, Output::AdvanceRefused(refused)
                if refused.payload().payload()
                    == &spirit::schema::signal::AdvanceRefusalReason::Unreachable),
            "an unreachable criome refuses the operation, got {reply:?}"
        );

        // The head did NOT advance — not even locally — and no trace exists
        // in the store or outbox.
        assert!(
            engine
                .versioned_log_head()
                .expect("versioned head reads")
                .is_none(),
            "a refused operation never advances the head"
        );
        assert_eq!(engine.record_count(), 0, "nothing was recorded anywhere");
        let handle = engine.store().engine_handle();
        assert!(
            handle.unshipped_outbox().expect("outbox reads").is_empty(),
            "a refused operation leaves no outbox trace"
        );
        assert_eq!(
            handle.store_durability().expect("durability reads"),
            Durability::ServerCommitted,
            "an empty log has nothing queued for the mirror"
        );

        // Reads stay served throughout.
        let version = engine.handle_async(Input::Version).await.into_root();
        assert!(
            matches!(version, Output::VersionReported(_)),
            "reads stay admitted while authorization is enabled, got {version:?}"
        );
    });
}
