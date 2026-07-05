//! THE OFFLINE FULL-CHAIN E2E WITNESS (designer report 669/2, P5 of 669).
//!
//! This is the first test that makes the WHOLE replication path true at once:
//!
//!   spirit source engine records intent
//!     → ships its versioned-log suffix to mirror A             (leg 1)
//!     → criome authorizes the shipped head digest              (leg 2)
//!     → router fans the typed `{ Spirit, Head }` reference      (leg 3)
//!     → mirror restore is rejected when latest is not that head (leg 4)
//!
//! It joins TWO already-green legs in ONE binary — no new shipped contract:
//!
//!   - Leg 1 + leg 3 are the ship → `ServerCommitted` → `Restore` →
//!     identical-records spine of `mirror`'s `tests/end_to_end_arc.rs`, run
//!     against a real in-process mirror `Service` over loopback TCP frames.
//!   - Leg 2 is the real in-process `CriomeRoot` policy evaluator: the fixture
//!     builds a 2-of-3 threshold contract and evidence over the shipped head
//!     digest, then asks criome to authorize it.
//!   - Leg 3 is router's `AuthorizedObjectFanout`: B and C subscribe to
//!     `{ Spirit, Head }`, and router publishes the typed
//!     `AuthorizedObjectReference` projected from criome.
//!   - Leg 4 is the interim acquire-by-D check: mirror still restores "latest,"
//!     so when D2 becomes latest after D1 was delivered, the acquire rejects
//!     instead of silently accepting D2 as D1.
//!
//! Fully offline: loopback TCP for mirror, in-process criome, in-process router
//! fanout, no tailnet and no live criome socket.
//!
//! Mainline pin unification (operator handoff H4, report 672): the mirror and
//! router legs now come from their main branches. Their shared transitive crates
//! (triad-runtime, sema-engine, nota, signal-frame) also pin `branch=main`,
//! so Cargo unifies every shared crate onto one rev and the legs link in one
//! binary.

use std::net::SocketAddr;
use std::path::PathBuf;

use criome::ActorRef as CriomeActorRef;
use criome::actors::root::{Arguments as CriomeArguments, CriomeRoot, SubmitRequest};
use criome::language::{AttestedMomentStatement, OperationStatement};
use criome::master_key::MasterKey;
use criome::tables::StoreLocation;
use mirror::{
    ComponentShipper, Engine as MirrorEngine, MirrorTailnetClient, Service, ServiceLink,
    ShipOutcome, Store as MirrorStore,
};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use router::{
    ActorIdentifier, AttendAuthorizedObjects, PublishAuthorizedObjectReference, RouterRuntime,
};
use sema_engine::{
    Assertion, CommitSequence, Durability, Engine as ComponentEngine, EngineOpen, EngineRecord,
    FamilyDirectory, FamilyName, Mutation, PortableCheckpoint, QueryPlan, RecordKey, Retraction,
    RowMaterializer, SchemaHash, SchemaVersion, TableDescriptor, TableName, TableReference,
    VersionedCommitLogEntry, VersionedStoreName, VersioningPolicy,
};
use signal_criome::{
    AttestedMoment, AttestedMomentProposition, AuthorizationEvaluation, AuthorizedObjectInterest,
    AuthorizedObjectKind, AuthorizedObjectObservation, ComponentKind, ComponentObjectInterest,
    Contract, CriomeReply, CriomeRequest, EvaluationDecision, Evidence, Identity,
    IdentityRegistration, KeyPurpose, OperationDigest, PolicyMember, RequiredSignatureThreshold,
    Rule, SignatureEnvelope, SignatureScheme, StampedSignatureEnvelope, Threshold, TimeSignature,
    TimeWindow, TimestampNanos,
};
use signal_mirror::{
    Input as MirrorInput, Output as MirrorOutput, RestoreBundle, RestoreQuery, StoreName,
};
use signal_standard::{
    AuthorizedObjectInterest as RouterAuthorizedObjectInterest,
    AuthorizedObjectKind as RouterAuthorizedObjectKind, ComponentKind as RouterComponentKind,
    ComponentObjectInterest as RouterComponentObjectInterest, ObjectDigest as RouterObjectDigest,
};
use triad_runtime::kameo::actor::Spawn as MirrorSpawn;

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
// Leg 3 import: fetch checkpoint + suffix from mirror A and import into a fresh
// component engine — the "own fetch transport." Verbatim shape from
// `end_to_end_arc.rs::Restorer::import`.
// ---------------------------------------------------------------------------

fn import_restore_bundle(bundle: RestoreBundle, target: &mut ComponentEngine) {
    let checkpoint =
        PortableCheckpoint::from_bytes(bundle.checkpoint.artifact.payload().payload().to_vec())
            .decode()
            .expect("decode checkpoint artifact");
    let suffix: Vec<VersionedCommitLogEntry> = bundle
        .suffix()
        .iter()
        .map(|envelope| {
            rkyv::from_bytes::<VersionedCommitLogEntry, rkyv::rancor::Error>(
                envelope.payload.payload().payload(),
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

struct PolicySigner {
    identity: Identity,
    key: MasterKey,
}

impl PolicySigner {
    fn developer(name: &str) -> Self {
        Self {
            identity: Identity::developer(name.to_owned()),
            key: MasterKey::generate().expect("policy signer key generates"),
        }
    }

    fn cluster(name: &str) -> Self {
        Self {
            identity: Identity::cluster(name.to_owned()),
            key: MasterKey::generate().expect("timekeeper key generates"),
        }
    }

    fn identity(&self) -> Identity {
        self.identity.clone()
    }

    fn registration(&self) -> IdentityRegistration {
        IdentityRegistration::new(
            self.identity(),
            self.key.public_key(),
            self.key.fingerprint(),
            KeyPurpose::ReleaseAuthorization,
            None,
        )
    }

    fn sign_moment(&self, proposition: &AttestedMomentProposition) -> TimeSignature {
        TimeSignature {
            signer: self.identity(),
            envelope: SignatureEnvelope {
                scheme: SignatureScheme::Bls12_381MinPk,
                public_key: self.key.public_key(),
                signature: self.key.sign(
                    AttestedMomentStatement::new(proposition)
                        .to_signing_bytes()
                        .expect("moment statement signs")
                        .as_slice(),
                ),
            },
        }
    }

    fn sign_operation(
        &self,
        operation: &OperationDigest,
        stamp: &AttestedMoment,
    ) -> StampedSignatureEnvelope {
        let statement = OperationStatement::new(&self.identity, operation, stamp)
            .to_signing_bytes()
            .expect("operation statement signs");
        StampedSignatureEnvelope {
            stamp: stamp.clone(),
            envelope: SignatureEnvelope {
                scheme: SignatureScheme::Bls12_381MinPk,
                public_key: self.key.public_key(),
                signature: self.key.sign(&statement),
            },
        }
    }
}

struct CriomePolicyFixture {
    first: PolicySigner,
    second: PolicySigner,
    third: PolicySigner,
    timekeeper: PolicySigner,
}

impl CriomePolicyFixture {
    fn new() -> Self {
        Self {
            first: PolicySigner::developer("operator-one"),
            second: PolicySigner::developer("operator-two"),
            third: PolicySigner::developer("operator-three"),
            timekeeper: PolicySigner::cluster("timekeeper"),
        }
    }

    async fn register_identities(&self, root: &CriomeActorRef<CriomeRoot>) {
        for signer in [&self.first, &self.second, &self.third, &self.timekeeper] {
            let reply = root
                .ask(SubmitRequest::new(CriomeRequest::RegisterIdentity(
                    signer.registration(),
                )))
                .await
                .expect("identity registration reaches criome")
                .into_reply();
            assert!(matches!(reply, CriomeReply::IdentityReceipt(_)));
        }
    }

    async fn admit_contract(
        &self,
        root: &CriomeActorRef<CriomeRoot>,
    ) -> signal_criome::ContractDigest {
        let contract = Contract::root(Rule::Threshold(Threshold::new(
            RequiredSignatureThreshold::new(2),
            vec![
                PolicyMember::KeyMember(self.first.identity()),
                PolicyMember::KeyMember(self.second.identity()),
                PolicyMember::KeyMember(self.third.identity()),
            ],
        )));
        let reply = root
            .ask(SubmitRequest::new(CriomeRequest::AdmitContract(contract)))
            .await
            .expect("contract admission reaches criome")
            .into_reply();
        let CriomeReply::ContractAdmitted(admitted) = reply else {
            panic!("expected ContractAdmitted, got {reply:?}");
        };
        admitted.into_payload()
    }

    fn stamp(&self) -> AttestedMoment {
        let proposition = AttestedMomentProposition::new(
            TimeWindow {
                opens_at: TimestampNanos::new(10),
                closes_at: TimestampNanos::new(20),
            },
            RequiredSignatureThreshold::new(1),
            vec![self.timekeeper.identity()],
        );
        AttestedMoment::new(
            proposition.clone(),
            vec![self.timekeeper.sign_moment(&proposition)],
        )
    }

    fn evidence(&self, operation: OperationDigest, signer_count: usize) -> Evidence {
        let stamp = self.stamp();
        let signatures = [&self.first, &self.second, &self.third]
            .into_iter()
            .take(signer_count)
            .map(|signer| signer.sign_operation(&operation, &stamp))
            .collect();
        Evidence::new(
            ComponentKind::Spirit,
            operation,
            stamp,
            signatures,
            Vec::new(),
        )
    }

    async fn evaluate_head(
        &self,
        root: &CriomeActorRef<CriomeRoot>,
        contract: signal_criome::ContractDigest,
        head_digest: &OperationDigest,
        signer_count: usize,
    ) -> (signal_criome::AuthorizedObjectReference, EvaluationDecision) {
        let evidence = self.evidence(head_digest.clone(), signer_count);
        let authorized_head = signal_criome::AuthorizedObjectReference {
            component: ComponentKind::Spirit,
            digest: evidence.operation.object_digest().clone(),
            kind: AuthorizedObjectKind::Head,
        };
        let reply = root
            .ask(SubmitRequest::new(CriomeRequest::EvaluateAuthorization(
                AuthorizationEvaluation {
                    contract,
                    object: authorized_head.clone(),
                    evidence,
                },
            )))
            .await
            .expect("authorization evaluation reaches criome")
            .into_reply();
        let CriomeReply::AuthorizationEvaluated(evaluated) = reply else {
            panic!("expected AuthorizationEvaluated, got {reply:?}");
        };
        (authorized_head, evaluated.decision)
    }
}

struct RestoreCandidate {
    bundle: RestoreBundle,
    tip_digest: OperationDigest,
}

impl RestoreCandidate {
    fn from_bundle(bundle: RestoreBundle) -> Self {
        let last_entry = bundle
            .suffix()
            .last()
            .expect("restore candidate carries at least one suffix entry");
        Self {
            tip_digest: OperationDigest::from_bytes(last_entry.digest.as_bytes()),
            bundle,
        }
    }

    fn tip_matches(&self, reference: &signal_standard::AuthorizedObjectReference) -> bool {
        self.tip_digest.object_digest().as_str() == reference.digest.as_str()
    }

    fn into_bundle(self) -> RestoreBundle {
        self.bundle
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authorized_d1_head_routes_and_rejects_restore_latest_after_d2() {
    // === LEG 1: spirit source records intent → ship to mirror A ============
    let fixture = ComponentFixture::new();
    let (source, source_thoughts) = fixture.populate();

    let mirror_directory = tempfile::tempdir().expect("mirror temp dir");
    let (link, mirror_a_address) = running_mirror(&mirror_directory).await;

    // The owner registers the spirit store on mirror A's meta surface.
    let registered = link
        .meta(meta_signal_mirror::Input::RegisterStore(
            meta_signal_mirror::StoreRegistration {
                store: meta_signal_mirror::StoreName::new(COMPONENT_STORE_NAME.to_owned()),
                addressing: meta_signal_mirror::ContentAddressing::Opaque,
            },
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

    let d1_digest = OperationDigest::from_bytes(confirmed_head.entry_digest().bytes());

    // === LEG 2: criome admits policy and authorizes the shipped D1 head =====
    let criome_directory = tempfile::tempdir().expect("criome temp dir");
    let criome_root = CriomeRoot::start(CriomeArguments::new(StoreLocation::new(
        criome_directory.path().join("criome.sema"),
    )))
    .await
    .expect("criome root starts");
    let criome_policy = CriomePolicyFixture::new();
    criome_policy.register_identities(&criome_root).await;
    let contract_digest = criome_policy.admit_contract(&criome_root).await;

    let (unauthorized_head, unauthorized_decision) = criome_policy
        .evaluate_head(&criome_root, contract_digest.clone(), &d1_digest, 1)
        .await;
    assert_ne!(
        unauthorized_decision,
        EvaluationDecision::Authorized,
        "one signature must not satisfy the 2-of-3 contract"
    );

    let (authorized_head, authorized_decision) = criome_policy
        .evaluate_head(&criome_root, contract_digest.clone(), &d1_digest, 2)
        .await;
    assert_eq!(authorized_decision, EvaluationDecision::Authorized);
    assert_eq!(
        authorized_head.digest,
        *d1_digest.object_digest(),
        "criome authorizes the exact shipped D1 head digest"
    );
    assert_eq!(
        unauthorized_head, authorized_head,
        "the denied and admitted evaluations refer to the same requested D1 object"
    );

    let snapshot = criome_root
        .ask(SubmitRequest::new(CriomeRequest::ObserveAuthorizedObjects(
            AuthorizedObjectObservation {
                subscriber: Identity::agent("router-authorized-object-bridge".to_owned()),
                interest: AuthorizedObjectInterest::ComponentObject(ComponentObjectInterest {
                    component: ComponentKind::Spirit,
                    kind: AuthorizedObjectKind::Head,
                }),
            },
        )))
        .await
        .expect("criome authorized-object observation reaches root")
        .into_reply();
    let CriomeReply::AuthorizedObjectUpdateSnapshot(snapshot) = snapshot else {
        panic!("expected AuthorizedObjectUpdateSnapshot, got {snapshot:?}");
    };
    let updates = snapshot.into_updates();
    assert_eq!(
        updates.len(),
        1,
        "criome publishes only the authorized D1 object, not the denied attempt"
    );
    assert_eq!(updates[0].object, authorized_head);
    assert_eq!(updates[0].decision, EvaluationDecision::Authorized);

    // === LEG 3: router fans the typed authorized-object reference ==========
    let router = RouterRuntime::start().await;
    for subscriber in ["spirit-b", "spirit-c"] {
        let snapshot = router
            .ask(AttendAuthorizedObjects {
                subscriber: ActorIdentifier::new(subscriber),
                interest: RouterAuthorizedObjectInterest::ComponentObject(
                    RouterComponentObjectInterest::new(
                        RouterComponentKind::Spirit,
                        RouterAuthorizedObjectKind::Head,
                    ),
                ),
            })
            .await
            .expect("router accepts authorized-object attendance");
        assert!(snapshot.references.is_empty());
    }
    let standard_reference = signal_standard::AuthorizedObjectReference::new(
        RouterComponentKind::Spirit,
        RouterObjectDigest::new(authorized_head.digest.as_str()),
        RouterAuthorizedObjectKind::Head,
    );
    let publication = router
        .ask(PublishAuthorizedObjectReference {
            reference: standard_reference.clone(),
        })
        .await
        .expect("router accepts authorized-object publication");
    assert_eq!(publication.deliveries.len(), 2);
    assert!(
        publication
            .deliveries
            .iter()
            .all(|delivery| delivery.reference == standard_reference),
        "router carries the typed reference only; payload fetch stays with the receiver"
    );
    let unrelated = router
        .ask(PublishAuthorizedObjectReference {
            reference: signal_standard::AuthorizedObjectReference::new(
                RouterComponentKind::Persona,
                signal_standard::ObjectDigest::new("unrelated-persona-head"),
                RouterAuthorizedObjectKind::Head,
            ),
        })
        .await
        .expect("router accepts unrelated publication");
    assert!(
        unrelated.deliveries.is_empty(),
        "component/object filters suppress unrelated authorized objects"
    );

    // Positive control: before D2 exists, mirror's latest restore still matches
    // the delivered D1 reference.
    let restore_client = MirrorTailnetClient::new(mirror_a_address);
    let d1_bundle = match restore_client
        .exchange(MirrorInput::Restore(RestoreQuery::new(StoreName::new(
            COMPONENT_STORE_NAME.to_owned(),
        ))))
        .await
        .expect("restore call succeeds")
    {
        MirrorOutput::Restored(bundle) => bundle,
        other => panic!("expected Restored, got {other:?}"),
    };
    let d1_candidate = RestoreCandidate::from_bundle(d1_bundle);
    assert!(
        d1_candidate.tip_matches(&standard_reference),
        "restore-latest matches D1 before any newer head exists"
    );

    let mut target = fixture.open_fresh("component-restored");
    import_restore_bundle(d1_candidate.into_bundle(), &mut target);
    let target_thoughts = target
        .register_table(fixture.thought_descriptor())
        .expect("thoughts re-register against the restored catalog");

    // THE CAUSAL SEAM: before a newer head exists, mirror B restored exactly up
    // to the D1 head the router announced.
    assert_eq!(
        target.current_commit_sequence().expect("target cursor"),
        CommitSequence::new(confirmed_head.commit_sequence().value()),
        "mirror B restored up to the D1 head the router announced"
    );

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
            Thought::new("gamma", "third"),
        ]
    );

    // Now make D2 latest after D1 was delivered. The current mirror API can
    // only restore latest, so an acquire driven by D1 must reject the candidate
    // instead of silently accepting D2 as D1.
    shipper
        .engine()
        .assert(Assertion::new(
            source_thoughts,
            Thought::new("delta", "fourth"),
        ))
        .expect("assert delta");
    let newer_head = match shipper
        .ship_unshipped()
        .await
        .expect("shipper ships D2 suffix")
    {
        ShipOutcome::Shipped { head } => head,
        other => panic!("expected D2 shipped history, got {other:?}"),
    };
    let d2_digest = OperationDigest::from_bytes(newer_head.entry_digest().bytes());
    assert_ne!(d2_digest, d1_digest, "D2 must be a distinct head");

    let d2_bundle = match restore_client
        .exchange(MirrorInput::Restore(RestoreQuery::new(StoreName::new(
            COMPONENT_STORE_NAME.to_owned(),
        ))))
        .await
        .expect("restore-latest after D2 succeeds")
    {
        MirrorOutput::Restored(bundle) => bundle,
        other => panic!("expected Restored, got {other:?}"),
    };
    let d2_candidate = RestoreCandidate::from_bundle(d2_bundle);
    assert_eq!(
        d2_candidate.tip_digest, d2_digest,
        "mirror latest now resolves to D2"
    );
    assert!(
        !d2_candidate.tip_matches(&standard_reference),
        "D1-driven acquire must reject restore-latest once D2 is latest"
    );

    router
        .stop_gracefully()
        .await
        .expect("router runtime stops gracefully");
    router.wait_for_shutdown().await;
    CriomeRoot::stop(criome_root)
        .await
        .expect("criome root stops");
}
