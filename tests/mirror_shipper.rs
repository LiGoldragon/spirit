//! The gated mirror shipper witness (Spirit 29pb, spirit side).
//!
//! Two proofs:
//!
//! (a) WITH a meta `MirrorTarget` configured against a running in-process
//!     mirror fixture, the daemon's post-commit drain ships the versioned-log
//!     suffix over real loopback TCP: the mirror persists it, acknowledges a
//!     head, the shared engine marks the shipped history `ServerCommitted`,
//!     and a FRESH spirit store restored from the mirror reads identical
//!     records.
//!
//! (b) UNSET — no `MirrorTarget` — nothing ships: the shipper never arms, the
//!     drain is a no-op, the store stays `QueuedForMirror`, and the daemon
//!     behaves exactly as one built before mirroring existed.

mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use support::domain_fixtures;

use mirror::{Engine as MirrorEngine, MirrorTailnetClient as TailnetClient, Service, ServiceLink};
use sema_engine::{Durability, EntryDigest};
use signal_mirror::{Input as MirrorInput, Output as MirrorOutput, RestoreQuery, StoreName};
use spirit::schema::meta_signal::{
    ArchiveDatabaseTarget, ConfigureRequest, MirrorAddress, MirrorAddressText, MirrorTarget,
    Output as MetaOutput,
};
use spirit::schema::signal::{
    Description, Domains, Entry, Importance, Input, Justification, Kind, Magnitude, Output,
    QuoteText, Reasoning, RecordRequest, Testimony, VerbatimQuote,
};
use spirit::{Engine, SPIRIT_STORE_NAME, Store, StoreError};
use tempfile::TempDir;
use triad_runtime::kameo::actor::Spawn;

const STORE_NAME: &str = SPIRIT_STORE_NAME;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn entry(description: &str) -> Entry {
    Entry {
        domains: domain_fixtures::domains(&["Information/Documentation"]),
        kind: Kind::Decision,
        description: Description::new(description),
        importance: Importance::new(Magnitude::Medium),
    }
}

fn record_request(description: &str) -> RecordRequest {
    RecordRequest {
        entry: entry(description),
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

/// Stand up an in-process mirror daemon runtime (real engine, real store, real
/// loopback TCP listener) and register the spirit store on its meta surface.
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

async fn restore_bundle_from_mirror(address: SocketAddr) -> signal_mirror::RestoreBundle {
    let client = TailnetClient::new(address);
    match client
        .exchange(MirrorInput::Restore(RestoreQuery::new(StoreName::new(
            STORE_NAME.to_owned(),
        ))))
        .await
        .expect("restore call succeeds")
    {
        MirrorOutput::Restored(bundle) => bundle,
        other => panic!("expected Restored, got {other:?}"),
    }
}

/// Restore a fresh spirit store from the mirror: fetch the checkpoint plus the
/// versioned-log suffix and import them through the production head-verified
/// mirror restore path.
async fn restore_from_mirror(
    address: SocketAddr,
    path: PathBuf,
    expected_head: EntryDigest,
) -> Store {
    let bundle = restore_bundle_from_mirror(address).await;
    Store::import_mirror_restore_bundle(path, bundle, expected_head)
        .expect("head-verified import into fresh spirit store")
}

#[test]
fn configured_mirror_target_ships_commits_and_a_fresh_store_restores_identically() {
    let runtime = runtime();
    let mirror_directory = tempfile::tempdir().expect("mirror temp dir");
    let component_directory = tempfile::tempdir().expect("component temp dir");

    runtime.block_on(async {
        let (_link, address) = running_mirror(&mirror_directory).await;

        let store =
            Store::open(component_directory.path().join("source.sema")).expect("open spirit store");
        let mut engine = Engine::new(store);
        engine.start().expect("engine starts");

        // OWNER configures the mirror target on the meta surface — this arms
        // the shipper against the live engine handle.
        let configured = engine.configure(ConfigureRequest::new(
            ArchiveDatabaseTarget::Default,
            Some(mirror_target(address)),
            None,
            None,
        ));
        assert!(
            matches!(configured, MetaOutput::Configured(_)),
            "configure accepted, got {configured:?}"
        );
        assert!(engine.mirror_shipping_armed(), "the shipper is armed");

        // A working write lands locally, then a local checkpoint, then a
        // post-checkpoint write — so the mirror restore carries both a
        // checkpoint body and a live suffix.
        record(&mut engine, "the log is authoritative").await;
        engine
            .store()
            .checkpoint()
            .expect("local checkpoint writes");
        record(&mut engine, "the fold materializes the view").await;

        // Before the drain, the local history is queued for the mirror.
        let handle = engine.store().engine_handle();
        assert_eq!(
            handle.store_durability().expect("durability reads"),
            Durability::QueuedForMirror
        );

        // DRAIN: exactly what the daemon's `handle_working_input` hook calls.
        let confirmed_head = match engine
            .ship_unshipped_to_mirror()
            .await
            .expect("ship unshipped")
            .expect("an armed shipper ships")
        {
            mirror::ShipOutcome::Shipped { head } => head,
            other => panic!("history shipped, got {other:?}"),
        };

        // The shared engine marks the shipped history server-committed and the
        // outbox is drained.
        assert_eq!(
            handle.store_durability().expect("durability reads"),
            Durability::ServerCommitted
        );
        assert!(
            handle.unshipped_outbox().expect("outbox reads").is_empty(),
            "the shipped cursor covers the whole outbox"
        );

        // Publish the checkpoint the restorer fetches, then restore a fresh
        // spirit store from the mirror and assert the query surface matches.
        assert!(
            engine
                .publish_checkpoint_to_mirror()
                .await
                .expect("publish checkpoint"),
            "an armed shipper publishes the checkpoint"
        );
        let wrong_head = EntryDigest::new([0; 32]);
        let wrong_restore = Store::import_mirror_restore_bundle(
            component_directory.path().join("wrong-head.sema"),
            restore_bundle_from_mirror(address).await,
            wrong_head,
        );
        assert!(
            matches!(
                wrong_restore,
                Err(StoreError::MirrorRestoreHeadMismatch { expected, .. }) if expected == wrong_head
            ),
            "a restore bundle cannot import under a mismatched expected head"
        );
        let restored = restore_from_mirror(
            address,
            component_directory.path().join("restored.sema"),
            confirmed_head.entry_digest(),
        )
        .await;

        assert_eq!(restored.len(), engine.store().len());
        assert_eq!(restored.database_marker(), engine.store().database_marker());
    });
}

#[test]
fn unconfigured_mirror_target_ships_nothing_and_leaves_behavior_unchanged() {
    let runtime = runtime();
    let component_directory = tempfile::tempdir().expect("component temp dir");

    runtime.block_on(async {
        let store =
            Store::open(component_directory.path().join("source.sema")).expect("open spirit store");
        let mut engine = Engine::new(store);
        engine.start().expect("engine starts");

        // No mirror target: the default Configure leaves the shipper unarmed.
        let configured = engine.configure(ConfigureRequest::new(
            ArchiveDatabaseTarget::Default,
            None,
            None,
            None,
        ));
        assert!(matches!(configured, MetaOutput::Configured(_)));
        assert!(
            !engine.mirror_shipping_armed(),
            "no target means no shipper"
        );

        record(&mut engine, "an unmirrored intent").await;

        // The drain is a no-op: nothing ships, the local history stays queued
        // for a mirror that was never configured.
        let shipped = engine
            .ship_unshipped_to_mirror()
            .await
            .expect("drain succeeds");
        assert!(shipped.is_none(), "an unarmed shipper ships nothing");

        let handle = engine.store().engine_handle();
        assert_eq!(
            handle.store_durability().expect("durability reads"),
            Durability::QueuedForMirror,
            "behavior is identical to a daemon with no mirroring"
        );
        assert!(
            !handle.unshipped_outbox().expect("outbox reads").is_empty(),
            "the write stays in the outbox, unshipped"
        );
    });
}
