//! THE OFFLINE FULL-CHAIN E2E WITNESS (designer report 669/2, P5 of 669).
//!
//! This is the first test that makes the WHOLE replication path true at once:
//!
//!   spirit source engine records intent
//!     → ships its versioned-log suffix to mirror A          (leg 1)
//!     → a router typed authorized-head reference reaches B/C  (leg 2)
//!     → mirror B fetches from mirror A and restores
//!       only if the restored head matches the reference.     (leg 3)
//!
//! It joins TWO already-green legs in ONE binary — no new shipped contract:
//!
//!   - Leg 1 + leg 3 are the ship → `ServerCommitted` → `Restore` →
//!     identical-records spine of `mirror`'s `tests/end_to_end_arc.rs`, run
//!     against a real in-process mirror `Service` over loopback TCP frames.
//!   - Leg 2 is router's typed authorized-object fan-out: B/C attend
//!     `{Spirit, Head}`, A publishes the authorized reference, and router
//!     returns the reference-only deliveries.
//!
//! The causal seam is the typed authorized-object reference. The current mirror
//! restore surface is still store-name-only, so the single-host witness adopts
//! the interim rule from report 700: restore latest, then reject if the restored
//! tip digest differs from the router-delivered digest. The falsifiable proof is
//! D1 delivered, D2 made latest, and D1 acquisition rejected rather than
//! silently accepting D2.
//!
//! Fully offline: loopback TCP for the mirror service, no tailnet, no criome
//! daemon. The criome authorization step is represented by the typed reference
//! entering router fan-out; wiring a live criome socket is the next slice.
//!
//! Mainline pin unification (operator handoff H4, report 672): the mirror and
//! router legs now come from their main branches. Their shared transitive crates
//! (triad-runtime, sema-engine, nota-next, signal-frame) also pin `branch=main`,
//! so Cargo unifies every shared crate onto one rev and the legs link in one
//! binary.

use std::net::SocketAddr;
use std::path::PathBuf;

use mirror::{
    ComponentShipper, Engine as MirrorEngine, MirrorTailnetClient, Service, ServiceLink,
    ShipOutcome, Store as MirrorStore,
};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use router::{
    ActorIdentifier, ActorRef, AttendAuthorizedObjects, PublishAuthorizedObjectReference,
    RouterRuntime,
};
use sema_engine::{
    Assertion, Durability, Engine as ComponentEngine, EngineOpen, EngineRecord, FamilyDirectory,
    FamilyName, MirrorHead, Mutation, QueryPlan, RecordKey, Retraction, RowMaterializer,
    SchemaHash, SchemaVersion, TableDescriptor, TableName, TableReference, VersionedStoreName,
    VersioningPolicy,
};
use signal_mirror::{
    Input as MirrorInput, Output as MirrorOutput, RestoreBundle, RestoreQuery, StoreName,
};
use signal_standard::{
    AuthorizedObjectInterest, AuthorizedObjectKind, AuthorizedObjectReference, ComponentKind,
    ComponentObjectInterest, ObjectDigest,
};
use triad_runtime::kameo::actor::Spawn;

const COMPONENT_STORE_NAME: &str = "full-chain-witness";

// ---------------------------------------------------------------------------
// The component domain record (the mirror never decodes it) — verbatim from
// `end_to_end_arc.rs`. A stand-in for a spirit intent Entry; the mirror is
// payload-blind, so any EngineRecord proves the path.
// ---------------------------------------------------------------------------

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Debug, Clone, PartialEq, Eq)]
#[rkyv(derive(Debug))]
struct Thought {
    key: String,
    body: String,
}

impl Thought {
    fn new(key: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            body: body.into(),
        }
    }
}

impl EngineRecord for Thought {
    fn record_key(&self) -> RecordKey {
        RecordKey::new(self.key.clone())
    }
}

struct Families {
    thoughts: TableReference<Thought>,
}

impl Families {
    fn new() -> Self {
        Self {
            thoughts: TableReference::new(TableName::new("thoughts")),
        }
    }
}

impl FamilyDirectory for Families {
    fn materialize(&self, row: RowMaterializer<'_>) -> sema_engine::Result<()> {
        match row.family().family().as_str() {
            "thought" => row.apply(self.thoughts),
            other => Err(sema_engine::Error::FamilyUnknown {
                family: other.to_owned(),
            }),
        }
    }
}

struct ComponentFixture {
    directory: tempfile::TempDir,
}

impl ComponentFixture {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(format!("{name}.sema"))
    }

    fn open_fresh(&self, file: &str) -> ComponentEngine {
        ComponentEngine::open(
            EngineOpen::new(self.path(file), SchemaVersion::new(1)).with_versioning(
                VersioningPolicy::new(VersionedStoreName::new(COMPONENT_STORE_NAME)),
            ),
        )
        .expect("component engine opens")
    }

    fn thought_descriptor(&self) -> TableDescriptor<Thought> {
        TableDescriptor::new(
            TableName::new("thoughts"),
            FamilyName::new("thought"),
            SchemaHash::for_label("thought-v1"),
        )
    }

    fn open_component(&self, file: &str) -> (ComponentEngine, TableReference<Thought>) {
        let mut engine = self.open_fresh(file);
        let thoughts = engine
            .register_table(self.thought_descriptor())
            .expect("thoughts register");
        (engine, thoughts)
    }

    /// Populate the spirit source: writes, a mid-history checkpoint, then
    /// post-checkpoint writes including a tombstone — same shape as
    /// `end_to_end_arc.rs::populate`, so the mirror restore carries a checkpoint
    /// body AND a live suffix.
    fn populate(&self) -> (ComponentEngine, TableReference<Thought>) {
        let (engine, thoughts) = self.open_component("component-source");
        engine
            .assert(Assertion::new(thoughts, Thought::new("alpha", "first")))
            .expect("assert alpha");
        engine
            .assert(Assertion::new(thoughts, Thought::new("beta", "second")))
            .expect("assert beta");
        engine
            .mutate(Mutation::new(thoughts, Thought::new("alpha", "revised")))
            .expect("mutate alpha");
        engine.checkpoint().expect("checkpoint writes");
        engine
            .assert(Assertion::new(thoughts, Thought::new("gamma", "third")))
            .expect("assert gamma");
        engine
            .retract(Retraction::new(thoughts, RecordKey::new("beta")))
            .expect("retract beta");
        (engine, thoughts)
    }
}

// ---------------------------------------------------------------------------
// Mirror A: a real in-process mirror Service over loopback TCP — the VC remote
// leg 1 ships into and leg 3 fetches from. Verbatim from `end_to_end_arc.rs`.
// ---------------------------------------------------------------------------

async fn running_mirror(directory: &tempfile::TempDir) -> (ServiceLink, SocketAddr) {
    let store =
        MirrorStore::open(&directory.path().join("mirror.sema")).expect("mirror store opens");
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
    (link, address)
}

// ---------------------------------------------------------------------------
// Router typed authorized-object fan-out. This is the report-700 interim:
// reference-only delivery is in-process; cross-socket router push is a
// follow-on.
// ---------------------------------------------------------------------------

async fn attend_spirit_heads(runtime: &ActorRef<RouterRuntime>, name: &str) {
    runtime
        .ask(AttendAuthorizedObjects {
            subscriber: ActorIdentifier::new(name),
            interest: AuthorizedObjectInterest::ComponentObject(ComponentObjectInterest::new(
                ComponentKind::Spirit,
                AuthorizedObjectKind::Head,
            )),
        })
        .await
        .expect("router accepts authorized-object attendance");
}

fn reference_for_head(head: &MirrorHead) -> AuthorizedObjectReference {
    AuthorizedObjectReference::new(
        ComponentKind::Spirit,
        ObjectDigest::new(HexDigest::from_bytes(head.entry_digest().bytes()).into_string()),
        AuthorizedObjectKind::Head,
    )
}

// ---------------------------------------------------------------------------
// Leg 3 import: fetch checkpoint + suffix from mirror A and import into a fresh
// component engine. The verifier rejects if restore-latest returns a head other
// than the router-delivered digest.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct HexDigest(String);

impl HexDigest {
    fn from_bytes(bytes: &[u8; 32]) -> Self {
        let mut text = String::with_capacity(64);
        for byte in bytes {
            text.push_str(&format!("{byte:02x}"));
        }
        Self(text)
    }

    fn into_string(self) -> String {
        self.0
    }
}

struct RestoreAttempt {
    bundle: RestoreBundle,
    expected: AuthorizedObjectReference,
}

impl RestoreAttempt {
    fn new(bundle: RestoreBundle, expected: AuthorizedObjectReference) -> Self {
        Self { bundle, expected }
    }

    fn restored_digest(&self) -> ObjectDigest {
        let bytes = self
            .bundle
            .suffix()
            .last()
            .map(|envelope| envelope.digest.as_bytes())
            .expect("restore bundle has a suffix head");
        ObjectDigest::new(HexDigest::from_bytes(bytes).into_string())
    }

    fn import_into(self, target: &mut ComponentEngine) -> Result<(), RestoreMismatch> {
        let restored = self.restored_digest();
        if restored != self.expected.digest {
            return Err(RestoreMismatch {
                expected: self.expected.digest,
                restored,
            });
        }
        self.import_verified_into(target);
        Ok(())
    }

    fn import_verified_into(self, target: &mut ComponentEngine) {
        let bundle = self.bundle;
        let checkpoint = sema_engine::PortableCheckpoint::from_bytes(
            bundle.checkpoint.artifact.as_slice().to_vec(),
        )
        .decode()
        .expect("decode checkpoint artifact");
        let suffix: Vec<sema_engine::VersionedCommitLogEntry> = bundle
            .suffix()
            .iter()
            .map(|envelope| {
                rkyv::from_bytes::<sema_engine::VersionedCommitLogEntry, rkyv::rancor::Error>(
                    envelope.payload.as_slice(),
                )
                .expect("decode versioned entry payload")
            })
            .collect();
        let mut session = target.begin_import().expect("import session mints");
        session
            .ingest_checkpoint(checkpoint)
            .expect("checkpoint ingests");
        session.ingest_suffix(suffix);
        session
            .commit(&Families::new())
            .expect("import commits into the fresh store");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RestoreMismatch {
    expected: ObjectDigest,
    restored: ObjectDigest,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn intent_recorded_on_node_a_ships_notifies_over_router_and_restores_identically_on_node_b() {
    // === LEG 1: spirit source records intent → ship to mirror A ============
    let fixture = ComponentFixture::new();
    let (source, source_thoughts) = fixture.populate();

    let mirror_directory = tempfile::tempdir().expect("mirror temp dir");
    let (link, mirror_a_address) = running_mirror(&mirror_directory).await;

    // The owner registers the spirit store on mirror A's meta surface.
    let registered = link
        .meta(meta_signal_mirror::Input::RegisterStore(
            meta_signal_mirror::StoreRegistration::new(meta_signal_mirror::StoreName::new(
                COMPONENT_STORE_NAME.to_owned(),
            )),
        ))
        .await
        .expect("meta register");
    assert!(matches!(
        registered,
        meta_signal_mirror::Output::StoreRegistered(_)
    ));

    // Before shipping: the local history is queued for the mirror.
    assert_eq!(
        source.store_durability().expect("durability reads"),
        Durability::QueuedForMirror
    );

    // SHIP: outbox suffix → envelopes → real TCP frames → mirror persists →
    // acknowledged head → ServerCommitted. The production ComponentShipper.
    let shipper = ComponentShipper::new(
        source,
        mirror_a_address,
        VersionedStoreName::new(COMPONENT_STORE_NAME),
    );
    let confirmed_head = match shipper
        .ship_unshipped()
        .await
        .expect("shipper ships unshipped suffix")
    {
        ShipOutcome::Shipped { head } => head,
        other => panic!("expected shipped history, got {other:?}"),
    };
    assert_eq!(
        shipper
            .engine()
            .store_durability()
            .expect("durability reads"),
        Durability::ServerCommitted
    );
    assert_eq!(
        shipper
            .engine()
            .durability_of(confirmed_head.commit_sequence())
            .expect("per-entry durability reads"),
        Durability::ServerCommitted
    );
    assert!(
        shipper
            .engine()
            .unshipped_outbox()
            .expect("outbox reads")
            .is_empty(),
        "the shipped cursor covers the whole outbox"
    );

    // Publish the checkpoint the restorer fetches.
    let checkpoint_receipt = shipper
        .publish_latest_checkpoint()
        .await
        .expect("checkpoint publishes");
    assert_eq!(*checkpoint_receipt.sequence.payload(), 1);
    assert_eq!(*checkpoint_receipt.covered_end.payload(), 3);

    let delivered_reference = reference_for_head(&confirmed_head);
    let delivered_digest = delivered_reference.digest.clone();

    // === LEG 2: router fans the typed authorized-head reference ============
    let router = RouterRuntime::start().await;
    attend_spirit_heads(&router, "spirit-b").await;
    attend_spirit_heads(&router, "spirit-c").await;
    let publication = router
        .ask(PublishAuthorizedObjectReference {
            reference: delivered_reference.clone(),
        })
        .await
        .expect("router publishes authorized object reference");
    assert_eq!(publication.deliveries.len(), 2);
    assert!(
        publication.deliveries.iter().any(|delivery| {
            delivery.subscriber == ActorIdentifier::new("spirit-b")
                && delivery.reference == delivered_reference
        }),
        "spirit-b receives the delivered authorized head"
    );
    assert!(
        publication.deliveries.iter().any(|delivery| {
            delivery.subscriber == ActorIdentifier::new("spirit-c")
                && delivery.reference == delivered_reference
        }),
        "spirit-c receives the delivered authorized head"
    );

    // === LEG 3: mirror B fetch + restore, DRIVEN by the witnessed notice ===
    // First prove C7: if D1 is delivered and D2 becomes latest before
    // acquisition, restore-latest must reject rather than silently accepting D2.
    shipper
        .engine()
        .assert(Assertion::new(
            source_thoughts,
            Thought::new("delta", "post-reference"),
        ))
        .expect("assert delta after D1 delivery");
    let latest_head = match shipper.ship_unshipped().await.expect("shipper ships D2") {
        ShipOutcome::Shipped { head } => head,
        other => panic!("expected shipped D2, got {other:?}"),
    };
    assert_ne!(
        HexDigest::from_bytes(latest_head.entry_digest().bytes()).into_string(),
        delivered_digest.payload().clone(),
        "D2 must differ from the delivered D1"
    );

    let restore_client = MirrorTailnetClient::new(mirror_a_address);
    let bundle = match restore_client
        .exchange(MirrorInput::Restore(RestoreQuery::new(StoreName::new(
            COMPONENT_STORE_NAME.to_owned(),
        ))))
        .await
        .expect("restore call succeeds")
    {
        MirrorOutput::Restored(bundle) => bundle,
        other => panic!("expected Restored, got {other:?}"),
    };
    assert_eq!(bundle.suffix().len(), 3, "D1 suffix plus post-reference D2");
    let mismatch = RestoreAttempt::new(bundle, delivered_reference.clone())
        .import_into(&mut fixture.open_fresh("component-restored-mismatch"))
        .expect_err("restore-latest must reject when it restored D2 for D1");
    assert_eq!(mismatch.expected, delivered_digest);
    assert_eq!(
        mismatch.restored,
        ObjectDigest::new(HexDigest::from_bytes(latest_head.entry_digest().bytes()).into_string())
    );

    // A fresh reference for the latest head succeeds with the same
    // restore-latest transport, proving the rejection above was specifically the
    // delivered-D mismatch gate.
    let latest_reference = reference_for_head(&latest_head);
    let latest_bundle = match restore_client
        .exchange(MirrorInput::Restore(RestoreQuery::new(StoreName::new(
            COMPONENT_STORE_NAME.to_owned(),
        ))))
        .await
        .expect("latest restore call succeeds")
    {
        MirrorOutput::Restored(bundle) => bundle,
        other => panic!("expected latest Restored, got {other:?}"),
    };

    let mut target = fixture.open_fresh("component-restored");
    RestoreAttempt::new(latest_bundle, latest_reference)
        .import_into(&mut target)
        .expect("latest delivered head imports");
    let target_thoughts = target
        .register_table(fixture.thought_descriptor())
        .expect("thoughts re-register against the restored catalog");

    // THE CAUSAL SEAM: mirror B restored only after the restored head matched
    // the router reference.
    assert_eq!(
        target.current_commit_sequence().expect("target cursor"),
        latest_head.commit_sequence(),
        "mirror B restored up to the latest matching reference"
    );

    // The normal query surface reads identical records on both engines.
    let source_records = shipper
        .engine()
        .match_records(QueryPlan::all(source_thoughts))
        .expect("source query")
        .records()
        .to_vec();
    let target_records = target
        .match_records(QueryPlan::all(target_thoughts))
        .expect("target query")
        .records()
        .to_vec();
    assert_eq!(source_records, target_records);
    assert_eq!(
        target_records,
        vec![
            Thought::new("alpha", "revised"),
            Thought::new("delta", "post-reference"),
            Thought::new("gamma", "third"),
        ]
    );

    // The restored store continues the same digest chain on B.
    assert_eq!(
        shipper
            .engine()
            .current_commit_sequence()
            .expect("source cursor"),
        target.current_commit_sequence().expect("target cursor"),
    );

    let _ = router.stop_gracefully().await;
    router.wait_for_shutdown().await;
}
