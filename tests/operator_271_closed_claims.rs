//! Architectural-truth witnesses for the closed claims in operator 271
//! `reports/operator/271-context-maintenance-current-state-2026-06-01.md`.
//!
//! Coverage in this file:
//! - Claim 4 — strict schema syntax and honest enum bodies CLOSED.
//!   The production-path plane schemas carry compact root-header object
//!   names (`Record Observe Lookup ...`, `WriteInput ReadInput`) and define
//!   those exported objects in the namespace (`RecordInput RecordRequest`, `Observe
//!   Query`, ...). Namespace enums carry explicit payload nouns such as `RecordInput` and
//!   direct non-same-named payload rows. The retired `Record@Entry`
//!   short-suffix sugar is absent.
//!   The authored schema source decodes into a typed `SchemaSource` value,
//!   lowers to semantic `TrueSchema`, round-trips through rkyv, and the emitted
//!   Rust carries the alias shape.
//!
//! Spirit-next is the production-pilot consumer of schema-emitted nouns,
//! so its source schema is the production witness for claim 4: the schema
//! the daemon and CLI binaries actually use must read as honest variants.
//!
//! Behavioural witnesses for the schema-emitted plane chain live in
//! `tests/generated_signal_plane.rs` and `tests/runtime_triad.rs`.

const SIGNAL_SCHEMA: &str = signal_spirit::SIGNAL_SCHEMA_SOURCE;
const DOMAIN_SCHEMA: &str = signal_spirit::DOMAIN_SCHEMA_SOURCE;
const NEXUS_SCHEMA: &str = include_str!("../schema/nexus.schema");
const SEMA_SCHEMA: &str = include_str!("../schema/sema.schema");

use schema_language::{
    ImportResolver, SchemaEngine, SchemaIdentity, SchemaSourceArtifact, TrueSchema,
};

/// Helper noun for schema-source assertions. Owns the source string and
/// the witness verbs.
struct SchemaSourceWitness<'source> {
    name: &'source str,
    source: &'source str,
}

impl<'source> SchemaSourceWitness<'source> {
    fn new(name: &'source str, source: &'source str) -> Self {
        Self { name, source }
    }

    fn must_contain(&self, needle: &str, claim: &str) {
        assert!(
            self.source.contains(needle),
            "claim {claim}: {} must contain {needle:?}",
            self.name
        );
    }

    fn must_not_contain(&self, needle: &str, claim: &str) {
        assert!(
            !self.source.contains(needle),
            "claim {claim}: {} must not contain {needle:?}",
            self.name
        );
    }

    fn must_round_trip_as_schema_source(&self) {
        let artifact = SchemaSourceArtifact::from_schema_text(self.source)
            .unwrap_or_else(|error| panic!("{} must decode as SchemaSource: {error}", self.name));
        let binary = artifact
            .to_binary_bytes()
            .unwrap_or_else(|error| panic!("{} must archive as rkyv: {error}", self.name));
        let recovered = SchemaSourceArtifact::from_binary_bytes(&binary)
            .unwrap_or_else(|error| panic!("{} must decode from rkyv: {error}", self.name));
        assert_eq!(
            artifact, recovered,
            "{} must preserve the typed SchemaSource value through rkyv",
            self.name
        );
    }

    fn must_lower_to_true_schema(
        &self,
        identity: SchemaIdentity,
        resolver: &ImportResolver,
    ) -> TrueSchema {
        let artifact = SchemaSourceArtifact::from_schema_text(self.source)
            .unwrap_or_else(|error| panic!("{} must decode as SchemaSource: {error}", self.name));
        SchemaEngine::default()
            .lower_schema_source_with_resolver(artifact.source(), identity, resolver)
            .unwrap_or_else(|error| panic!("{} must lower to TrueSchema: {error}", self.name))
    }
}

/// Claim 4 — `signal-spirit/schema/signal.schema` declares the `Input` enum body with
/// compact exported object names. The retired `Record@Entry`
/// short-suffix sugar is absent from the active production schema, and
/// the payload shape lives in namespace declarations.
#[test]
fn signal_schema_input_uses_exported_object_variant_names() {
    let witness = SchemaSourceWitness::new("signal-spirit/schema/signal.schema", SIGNAL_SCHEMA);

    // The active production Input enum body — compact exported objects.
    witness.must_contain(
        "[(State StateInput) (Record RecordInput) (Propose ProposeInput) (Clarify ClarifyInput) (Supersede SupersedeInput) (Retire RetireInput) (ResolveClarification ResolveClarificationInput) (Observe ObserveInput) (PublicTextSearch PublicTextSearchInput) (PublicRecords PublicRecordsInput) (PrivateRecords PrivateRecordsInput) (Lookup LookupInput) (Count CountInput) (ChangeCertainty ChangeCertaintyInput) (BumpImportance BumpImportanceInput) (ChangeRecord ChangeRecordInput) (RegisterReferent RegisterReferentInput) (LookupStash LookupStashInput) (Tap TapInput) (Untap UntapInput) (ApplyAuthorizedRecord ApplyAuthorizedRecordInput) (SubscribeIntent SubscribeIntentInput opens IntentEventStream) Version Marker (PublicIntent PublicIntentInput)]",
        "4",
    );
    // The working hard-delete `Remove` and the working `CollectRemovalCandidates`
    // are retired: physical deletion is the owner-only meta op, no working verb.
    witness.must_not_contain(" Remove ", "4");
    witness.must_not_contain(" CollectRemovalCandidates ", "4");
    witness.must_not_contain("Remove Removal", "4");
    witness.must_not_contain("CollectRemovalCandidates RemovalCandidateCollection", "4");
    witness.must_contain("StateInput Statement", "4");
    witness.must_contain("RecordInput RecordRequest", "4");
    witness.must_contain("ProposeInput Proposal", "4");
    witness.must_contain("ClarifyInput Clarification", "4");
    witness.must_contain("ResolveClarificationInput ClarificationResolution", "4");
    witness.must_contain("SupersedeInput Supersession", "4");
    witness.must_contain("RetireInput Retirement", "4");
    witness.must_contain("ObserveInput Query", "4");
    witness.must_contain("PublicIntentInput DomainScopes", "4");
    witness.must_contain("PublicTextSearchInput SearchText", "4");
    witness.must_contain("PublicRecordsInput RecordSelection", "4");
    witness.must_contain("PrivateRecordsInput RecordSelection", "4");
    witness.must_contain("LookupInput RecordIdentifier", "4");
    witness.must_contain("CountInput Query", "4");
    witness.must_contain("ChangeCertaintyInput CertaintyChange", "4");
    witness.must_contain("BumpImportanceInput ImportanceBump", "4");
    witness.must_contain("ChangeRecordInput RecordChange", "4");
    witness.must_contain("RegisterReferentInput ReferentRegistration", "4");
    witness.must_contain("Justification { Testimony Reasoning }", "4");
    witness.must_contain("RecordRequest { Entry Justification }", "4");
    witness.must_contain("Proposal { Entry Justification }", "4");
    witness.must_contain("LookupStashInput StashHandle", "4");
    witness.must_contain("SubscribeIntentInput Query", "4");
    witness.must_contain("Version", "4");

    // Retired short-suffix shorthand must not appear.
    witness.must_not_contain("Record@Entry", "4");
    witness.must_not_contain("Observe@Query", "4");
    witness.must_not_contain("Remove@RecordIdentifier", "4");
    witness.must_not_contain("@Vec", "4");
    witness.must_not_contain("@Option", "4");
    witness.must_not_contain("@KeyValue", "4");
    witness.must_not_contain("@Map", "4");
}

#[test]
fn signal_schema_domains_put_software_under_the_software_branch() {
    let witness = SchemaSourceWitness::new("signal-spirit/schema/domain.schema", DOMAIN_SCHEMA);

    witness.must_contain(
        "CraftDomain [Electronics Construction Carpentry Metalworking Sewing Manufacturing Repair Engineering Handicraft Invention]",
        "software-domain",
    );
    witness.must_contain("SoftwareDomain [", "software-domain");
    witness.must_contain("TechnologyDomain [", "software-domain");
    witness.must_contain("(Hardware HardwareLeaf)", "software-domain");
    witness.must_contain("HardwareLeaf [All Networking]", "software-domain");
    witness.must_contain("(Programming ProgrammingLeaf)", "software-domain");
    witness.must_contain(
        "ProgrammingLeaf [All TypeSystems Compilation Parsing Grammars CodeGeneration Metaprogramming Macros DomainSpecificLanguages]",
        "software-domain",
    );
    witness.must_contain("(Quality QualityLeaf)", "software-domain");
    witness.must_contain("QualityLeaf [All Testing]", "software-domain");
    witness.must_contain("(Operations OperationsLeaf)", "software-domain");
    witness.must_contain(
        "OperationsLeaf [All BuildSystem ReleaseEngineering DependencyManagement Deployment ConfigurationManagement]",
        "software-domain",
    );
    witness.must_contain("(Engineering EngineeringLeaf)", "software-domain");
    witness.must_contain(
        "EngineeringLeaf [All Architecture Design ApplicationProgrammingInterfaces Documentation VersionControl DevelopmentProcess Management Modularity]",
        "software-domain",
    );
    witness.must_not_contain(
        "(Craft [Programming Architecture Schema Infrastructure Versioning Testing",
        "software-domain",
    );
    witness.must_contain(
        "(Equivalence [(Information Database) (Technology Software Data All)])",
        "software-domain",
    );
}

/// Claim 4 — `signal-spirit/schema/signal.schema` declares the `Output` enum body with
/// compact exported object names and namespace declarations.
#[test]
fn signal_schema_output_uses_exported_object_variant_names() {
    let witness = SchemaSourceWitness::new("signal-spirit/schema/signal.schema", SIGNAL_SCHEMA);

    // The active production Output enum body.
    witness.must_contain(
        "[(RecordAccepted RecordAcceptedOutput) (Proposed ProposedOutput) (Clarified ClarifiedOutput) (Superseded SupersededOutput) (Retired RetiredOutput) (ClarificationResolved ClarificationResolvedOutput) (GuardianRejected GuardianRejectedOutput) (ReferentGuardianRejected ReferentGuardianRejectedOutput) (RecordsObserved RecordsObservedOutput) (RecordsStashed RecordsStashedOutput) (RecordFound RecordFoundOutput) (RecordsCounted RecordsCountedOutput) (CertaintyChanged CertaintyChangedOutput) (ImportanceBumped ImportanceBumpedOutput) (RecordChanged RecordChangedOutput) (ReferentRegistered ReferentRegisteredOutput) (ObservationTapped ObservationTappedOutput) (ObservationUntapped ObservationUntappedOutput) (SubscriptionStarted SubscriptionStartedOutput) (VersionReported VersionReportedOutput) (MarkerReported MarkerReportedOutput) (RecordApplied RecordAppliedOutput) (ApplyRefused ApplyRefusedOutput) (Event IntentEvent) (Error ErrorOutput) (Rejected RejectedOutput) (AdvanceRefused AdvanceRefusedOutput)]",
        "4",
    );
    witness.must_not_contain("RecordRemoved", "4");
    witness.must_not_contain("RemovalCandidatesCollected", "4");
    witness.must_not_contain("RecordRemoved RemoveReceipt", "4");
    witness.must_contain("RecordAcceptedOutput RecordIdentifier", "4");
    witness.must_contain("ProposedOutput RecordIdentifier", "4");
    witness.must_contain("ClarifiedOutput ClarificationReceipt", "4");
    witness.must_contain(
        "ClarificationResolvedOutput ClarificationResolutionReceipt",
        "4",
    );
    witness.must_contain("SupersededOutput SupersessionReceipt", "4");
    witness.must_contain("RetiredOutput RetirementReceipt", "4");
    witness.must_contain("GuardianRejectedOutput GuardianRejection", "4");
    witness.must_contain(
        "ReferentGuardianRejectedOutput ReferentGuardianRejection",
        "4",
    );
    witness.must_contain("RecordsObservedOutput ObservedRecords", "4");
    witness.must_contain("RecordFoundOutput FoundRecord", "4");
    witness.must_contain("RecordsCountedOutput CountedRecords", "4");
    witness.must_contain("CertaintyChangedOutput CertaintyChangeReceipt", "4");
    witness.must_contain("ImportanceBumpedOutput ImportanceBumpReceipt", "4");
    witness.must_contain("RecordChangedOutput RecordChangeReceipt", "4");
    witness.must_contain("ReferentRegisteredOutput ReferentRegistrationReceipt", "4");
    witness.must_contain("SubscriptionStartedOutput IntentSubscription", "4");
    witness.must_contain("VersionReportedOutput VersionReport", "4");
    witness.must_contain("MarkerReportedOutput DatabaseMarker", "4");
    witness.must_contain("VersionReport { VersionText }", "4");
    witness.must_contain(
        "IntentEvent [(IntentRecorded IntentRecordedEvent belongs IntentEventStream) (IntentClarified IntentClarifiedEvent belongs IntentEventStream) (IntentSuperseded IntentSupersededEvent belongs IntentEventStream) (IntentRetired IntentRetiredEvent belongs IntentEventStream)]",
        "4",
    );
    witness.must_contain("AdvanceRefusedOutput AdvanceRefusal", "4");
    witness.must_contain("ErrorOutput ErrorReport", "4");
    witness.must_contain("RejectedOutput SignalRejection", "4");
}

/// Claim 4 — `signal-spirit/schema/signal.schema` declares the `ValidationError` enum body
/// with bare PascalCase unit variants. This is the honest form for
/// payload-free variants; no parens, no sigil.
#[test]
fn signal_schema_unit_variant_enum_uses_bare_pascal_case_atoms() {
    let witness = SchemaSourceWitness::new("signal-spirit/schema/signal.schema", SIGNAL_SCHEMA);

    // ValidationError carries bare unit variants per designer 480; keyword
    // and text-query validation add typed read-predicate failures.
    witness.must_contain(
        "ValidationError [EmptyDomain EmptyDescription EmptyQueryDomain EmptyKeyword EmptySearchText EmptyQueryReferent StashHandleNotFound EmptyReferents]",
        "4",
    );

    // Kind is a five-variant unit enum.
    witness.must_contain(
        "Kind [Decision Principle Correction Clarification Constraint]",
        "4",
    );

    // Magnitude is a seven-variant unit enum.
    witness.must_contain(
        "Magnitude [Zero Minimum VeryLow Low Medium High VeryHigh Maximum]",
        "4",
    );
}

/// Claim 4 — The whole production schema MUST be free of the retired `@`
/// sigil. This is the broader regression sweep — even a single `@`
/// character anywhere in the schema would resurrect the rejected shorthand.
#[test]
fn split_schemas_carry_no_at_sigil_anywhere() {
    let signal_witness =
        SchemaSourceWitness::new("signal-spirit/schema/signal.schema", SIGNAL_SCHEMA);
    let domain_witness =
        SchemaSourceWitness::new("signal-spirit/schema/domain.schema", DOMAIN_SCHEMA);
    let nexus_witness = SchemaSourceWitness::new("schema/nexus.schema", NEXUS_SCHEMA);
    let sema_witness = SchemaSourceWitness::new("schema/sema.schema", SEMA_SCHEMA);

    signal_witness.must_not_contain("@", "4");
    domain_witness.must_not_contain("@", "4");
    nexus_witness.must_not_contain("@", "4");
    sema_witness.must_not_contain("@", "4");
}

/// Claim 4 — The authored schemas are the durable schema values. Each
/// one decodes into `SchemaSource` and archives through rkyv without an
/// intermediate checked-in schema artifact between source and emitted Rust.
#[test]
fn split_schema_sources_decode_and_archive_as_typed_schema_values() {
    let signal_witness =
        SchemaSourceWitness::new("signal-spirit/schema/signal.schema", SIGNAL_SCHEMA);
    let domain_witness =
        SchemaSourceWitness::new("signal-spirit/schema/domain.schema", DOMAIN_SCHEMA);
    let nexus_witness = SchemaSourceWitness::new("schema/nexus.schema", NEXUS_SCHEMA);
    let sema_witness = SchemaSourceWitness::new("schema/sema.schema", SEMA_SCHEMA);

    signal_witness.must_round_trip_as_schema_source();
    domain_witness.must_round_trip_as_schema_source();
    nexus_witness.must_round_trip_as_schema_source();
    sema_witness.must_round_trip_as_schema_source();

    let dependency_resolver = ImportResolver::new()
        .with_module_source("signal-domain", "domain", "0.1.0", DOMAIN_SCHEMA)
        .with_module_source("signal-spirit", "signal", "0.13.0", SIGNAL_SCHEMA)
        .with_module_source("spirit", "sema", "0.6.0", SEMA_SCHEMA);
    let signal_true_schema = signal_witness.must_lower_to_true_schema(
        SchemaIdentity::new("signal-spirit:signal", "0.13.0"),
        &dependency_resolver,
    );
    let domain_true_schema = domain_witness.must_lower_to_true_schema(
        SchemaIdentity::new("signal-domain:domain", "0.1.0"),
        &dependency_resolver,
    );
    let sema_true_schema = sema_witness.must_lower_to_true_schema(
        SchemaIdentity::new("spirit:sema", "0.6.0"),
        &dependency_resolver,
    );
    let nexus_true_schema = nexus_witness.must_lower_to_true_schema(
        SchemaIdentity::new("spirit:nexus", "0.6.0"),
        &dependency_resolver,
    );
    assert!(
        signal_true_schema.resolved_imports().len() >= 40,
        "signal TrueSchema resolves shared domain imports rather than embedding the taxonomy"
    );
    assert!(
        !domain_true_schema.namespace().is_empty(),
        "domain TrueSchema keeps the taxonomy as semantic declarations"
    );
    assert_eq!(
        sema_true_schema.families().len(),
        3,
        "sema TrueSchema keeps record families as schema-derived storage declarations"
    );
    assert!(
        nexus_true_schema.resolved_imports().len() >= 40,
        "nexus TrueSchema resolves its signal and sema nouns through dependency imports"
    );

    signal_witness.must_contain(
        "[(State StateInput) (Record RecordInput) (Propose ProposeInput) (Clarify ClarifyInput) (Supersede SupersedeInput) (Retire RetireInput) (ResolveClarification ResolveClarificationInput) (Observe ObserveInput) (PublicTextSearch PublicTextSearchInput) (PublicRecords PublicRecordsInput) (PrivateRecords PrivateRecordsInput) (Lookup LookupInput) (Count CountInput) (ChangeCertainty ChangeCertaintyInput) (BumpImportance BumpImportanceInput) (ChangeRecord ChangeRecordInput) (RegisterReferent RegisterReferentInput) (LookupStash LookupStashInput) (Tap TapInput) (Untap UntapInput) (ApplyAuthorizedRecord ApplyAuthorizedRecordInput) (SubscribeIntent SubscribeIntentInput opens IntentEventStream) Version Marker (PublicIntent PublicIntentInput)]",
        "4",
    );
    signal_witness.must_contain("StateInput Statement", "4");
    signal_witness.must_contain("RecordInput RecordRequest", "4");
    signal_witness.must_contain("ObserveInput Query", "4");
    signal_witness.must_contain("PublicIntentInput DomainScopes", "4");
    signal_witness.must_contain("PublicTextSearchInput SearchText", "4");
    signal_witness.must_contain("PublicRecordsInput RecordSelection", "4");
    signal_witness.must_contain("PrivateRecordsInput RecordSelection", "4");
    signal_witness.must_contain("LookupInput RecordIdentifier", "4");
    signal_witness.must_contain("ChangeCertaintyInput CertaintyChange", "4");
    signal_witness.must_contain("BumpImportanceInput ImportanceBump", "4");
    signal_witness.must_contain("ChangeRecordInput RecordChange", "4");
    signal_witness.must_contain("RegisterReferentInput ReferentRegistration", "4");
    signal_witness.must_contain("SubscribeIntentInput Query", "4");
    signal_witness.must_contain("Version", "4");
    signal_witness.must_contain("Marker", "4");
    sema_witness.must_contain(
        "[(WriteInput WriteInputRoot) (ReadInput ReadInputRoot)]",
        "4",
    );
    sema_witness.must_contain(
        "WriteInput [(Record Entry) (ChangeCertainty CertaintyChange) (BumpImportance ImportanceBump) (ChangeRecord RecordChange) (RegisterReferent ReferentRegistration)]",
        "4",
    );
    sema_witness.must_contain(
        "ReadInput [(Observe Query) (PublicIntent DomainScopes) (PublicTextSearch SearchText) (Lookup RecordIdentifier) (Count Query)]",
        "4",
    );
    sema_witness.must_not_contain("(Remove)", "4");
    sema_witness.must_contain("Recorded SemaReceipt", "4");
    sema_witness.must_contain(
        "WriteOutput [(Recorded SemaReceipt) (CertaintyChanged CertaintyChangeReceipt) (ImportanceBumped ImportanceBumpReceipt) (RecordChanged RecordChangeReceipt) (ReferentRegistered ReferentRegistrationReceipt) (Missed ErrorReport)]",
        "4",
    );
    sema_witness.must_contain(
        "ReadOutput [(Observed ObservedRecords) (PublicIntentResults ObservedRecords) (PublicTextSearchResults ObservedRecords) (Found FoundRecord) (Counted CountedRecords) (Missed ErrorReport)]",
        "4",
    );
    sema_witness.must_not_contain("(Removed)", "4");
    nexus_witness.must_contain(
        "CommandSemaWrite [(Record Entry) (ChangeCertainty CertaintyChange) (BumpImportance ImportanceBump) (ChangeRecord RecordChange) (RegisterReferent ReferentRegistration)]",
        "4",
    );
    nexus_witness.must_contain("Work (Frame [Event WriteDone ReadDone EffectDone] [(SignalArrived Event) (SemaWriteCompleted WriteDone) (SemaReadCompleted ReadDone) (EffectCompleted EffectDone)])", "4");
    nexus_witness.must_contain("Action (Frame [Reply Write Read Effect Continuation] [(ReplyToSignal Reply) (CommandSemaWrite Write) (CommandSemaRead Read) (CommandEffect Effect) (Continue Continuation)])", "4");
    nexus_witness.must_contain(
        "NexusWork Work.(SignalInput SemaWriteOutput SemaReadOutput NexusEffectResult)",
        "4",
    );
    nexus_witness.must_contain(
        "NexusAction Action.(SignalOutput CommandSemaWrite SemaReadInput NexusEffectCommand NexusWork)",
        "4",
    );
    nexus_witness.must_contain(
        "NexusEffectCommand [(Stash StashRequest) (ClassifyState Statement) (RecordWithImpliedReferents RecordRequest) (GuardRecord RecordRequest) (ProposeWithImpliedReferents Proposal) (Propose Proposal) (Clarify Clarification) (SupersedeWithImpliedReferents Supersession) (Supersede Supersession) (Retire Retirement) (ResolveClarification ClarificationResolution) (ChangeRecordWithImpliedReferents RecordChange) (GuardChangeRecord RecordChange) (GuardReferentRegistration ReferentRegistration) (OpenIntentSubscription Query) (OpenObserverTap ObserverFilter) (CloseObserverTap SubscriptionToken)]",
        "4",
    );
    nexus_witness.must_not_contain("(GuardRemove)", "4");
    nexus_witness.must_not_contain("(CollectRemovalCandidates)", "4");
    nexus_witness.must_contain("ClassifyState Statement", "4");
    nexus_witness.must_contain("RecordWithImpliedReferents RecordRequest", "4");
    nexus_witness.must_contain("GuardRecord RecordRequest", "4");
    nexus_witness.must_contain("ProposeWithImpliedReferents Proposal", "4");
    nexus_witness.must_contain("Propose Proposal", "4");
    nexus_witness.must_contain("Clarify Clarification", "4");
    nexus_witness.must_contain("ResolveClarification ClarificationResolution", "4");
    nexus_witness.must_contain("SupersedeWithImpliedReferents Supersession", "4");
    nexus_witness.must_contain("Supersede Supersession", "4");
    nexus_witness.must_contain("Retire Retirement", "4");
    nexus_witness.must_not_contain("GuardRemove Removal", "4");
    nexus_witness.must_contain("ChangeRecordWithImpliedReferents RecordChange", "4");
    nexus_witness.must_contain("GuardChangeRecord RecordChange", "4");
    nexus_witness.must_contain("GuardReferentRegistration ReferentRegistration", "4");
    nexus_witness.must_contain("OpenIntentSubscription Query", "4");
    nexus_witness.must_contain(
        "NexusEffectResult [(Stashed StashResult) (StateClassified RecordRequest) (RecordReferentsSettled RecordRequest) (ProposeReferentsSettled Proposal) (SupersedeReferentsSettled Supersession) (ChangeRecordReferentsSettled RecordChange) (Recorded SemaReceipt) (Proposed SemaReceipt) (Clarified ClarificationReceipt) (Superseded SupersessionReceipt) (Retired RetirementReceipt) (ClarificationResolved ClarificationResolutionReceipt) (RecordChanged RecordChangeReceipt) (GuardianRejected GuardianRejection) (ReferentRegistered ReferentRegistrationReceipt) (ReferentGuardianRejected ReferentGuardianRejection) (OperationFailed ErrorReport) (IntentSubscriptionOpened IntentSubscription) (ObserverTapOpened ObserverSubscription) (ObserverTapClosed ObserverRetraction)]",
        "4",
    );
    nexus_witness.must_not_contain("(Removed)", "4");
    nexus_witness.must_not_contain("(RemovalCandidatesCollected)", "4");
    nexus_witness.must_contain("StateClassified RecordRequest", "4");
    nexus_witness.must_contain("RecordReferentsSettled RecordRequest", "4");
    nexus_witness.must_contain("ProposeReferentsSettled Proposal", "4");
    nexus_witness.must_contain("SupersedeReferentsSettled Supersession", "4");
    nexus_witness.must_contain("ChangeRecordReferentsSettled RecordChange", "4");
    nexus_witness.must_contain("ClarificationResolved ClarificationResolutionReceipt", "4");
    nexus_witness.must_contain("Recorded SemaReceipt", "4");
    nexus_witness.must_contain("Proposed SemaReceipt", "4");
    nexus_witness.must_contain("Clarified ClarificationReceipt", "4");
    nexus_witness.must_contain("Superseded SupersessionReceipt", "4");
    nexus_witness.must_contain("Retired RetirementReceipt", "4");
    nexus_witness.must_contain("GuardianRejected GuardianRejection", "4");
    nexus_witness.must_contain("ReferentRegistered ReferentRegistrationReceipt", "4");
    nexus_witness.must_contain("ReferentGuardianRejected ReferentGuardianRejection", "4");
    nexus_witness.must_contain("OperationFailed ErrorReport", "4");
    nexus_witness.must_contain("IntentSubscriptionOpened IntentSubscription", "4");
}

/// Claim 4 — The schema-emitted Rust source surface mirrors the honest
/// schema. The generated plane modules the spirit crate compiles against
/// MUST declare the same enums with the same variants. This is the
/// projection-side witness for claim 4.
#[test]
fn schema_emitted_rust_modules_mirror_honest_enum_variants() {
    let signal_rust = signal_spirit::SIGNAL_RUST_SOURCE;
    let nexus_rust = include_str!("../src/schema/nexus.rs");
    let sema_rust = include_str!("../src/schema/sema.rs");
    let signal_witness =
        SchemaSourceWitness::new("signal-spirit/src/schema/signal.rs", signal_rust);
    let nexus_witness = SchemaSourceWitness::new("src/schema/nexus.rs", nexus_rust);
    let sema_witness = SchemaSourceWitness::new("src/schema/sema.rs", sema_rust);

    // The schema-emitted Input enum carries exported wrapper nouns.
    signal_witness.must_contain("pub enum Input {", "4");
    signal_witness.must_contain("pub struct StateInput(Statement);", "4");
    signal_witness.must_contain("State(StateInput)", "4");
    signal_witness.must_contain("pub struct RecordInput(RecordRequest);", "4");
    signal_witness.must_contain("pub struct ProposeInput(Proposal);", "4");
    signal_witness.must_contain("pub struct ObserveInput(Query);", "4");
    signal_witness.must_contain("pub struct PublicIntentInput(DomainScopes);", "4");
    signal_witness.must_contain("pub struct PublicTextSearchInput(SearchText);", "4");
    signal_witness.must_contain("pub struct PublicRecordsInput(RecordSelection);", "4");
    signal_witness.must_contain("pub struct PrivateRecordsInput(RecordSelection);", "4");
    signal_witness.must_contain("pub struct LookupInput(RecordIdentifier);", "4");
    signal_witness.must_contain("Record(RecordInput)", "4");
    signal_witness.must_contain("Observe(ObserveInput)", "4");
    signal_witness.must_contain("PublicIntent(PublicIntentInput)", "4");
    signal_witness.must_contain("PublicTextSearch(PublicTextSearchInput)", "4");
    signal_witness.must_contain("PublicRecords(PublicRecordsInput)", "4");
    signal_witness.must_contain("PrivateRecords(PrivateRecordsInput)", "4");
    signal_witness.must_contain("Lookup(LookupInput)", "4");
    signal_witness.must_contain("Count(CountInput)", "4");
    signal_witness.must_not_contain("Remove(Remove)", "4");
    signal_witness.must_not_contain("CollectRemovalCandidates(CollectRemovalCandidates)", "4");
    signal_witness.must_contain("ChangeCertainty(ChangeCertaintyInput)", "4");
    signal_witness.must_contain("BumpImportance(BumpImportanceInput)", "4");
    signal_witness.must_contain("RegisterReferent(RegisterReferentInput)", "4");
    signal_witness.must_contain("Version", "4");
    signal_witness.must_contain("Marker", "4");

    // The schema-emitted Output enum carries exported wrapper nouns.
    signal_witness.must_contain("pub enum Output {", "4");
    signal_witness.must_contain("pub struct RecordAcceptedOutput(RecordIdentifier);", "4");
    signal_witness.must_contain("pub struct RecordsObservedOutput(ObservedRecords);", "4");
    signal_witness.must_contain("pub struct RecordsStashedOutput(StashedObservation);", "4");
    signal_witness.must_contain("RecordAccepted(RecordAcceptedOutput)", "4");
    signal_witness.must_contain("RecordsObserved(RecordsObservedOutput)", "4");
    signal_witness.must_contain("RecordsStashed(RecordsStashedOutput)", "4");
    signal_witness.must_contain("RecordFound(RecordFoundOutput)", "4");
    signal_witness.must_contain("RecordsCounted(RecordsCountedOutput)", "4");
    signal_witness.must_not_contain("RecordRemoved(RecordRemoved)", "4");
    signal_witness.must_not_contain(
        "RemovalCandidatesCollected(RemovalCandidatesCollected)",
        "4",
    );
    signal_witness.must_contain("CertaintyChanged(CertaintyChangedOutput)", "4");
    signal_witness.must_contain("ImportanceBumped(ImportanceBumpedOutput)", "4");
    signal_witness.must_contain("ReferentRegistered(ReferentRegisteredOutput)", "4");
    signal_witness.must_contain(
        "ReferentGuardianRejected(ReferentGuardianRejectedOutput)",
        "4",
    );
    signal_witness.must_contain("VersionReported(VersionReportedOutput)", "4");
    signal_witness.must_contain("MarkerReported(MarkerReportedOutput)", "4");
    signal_witness.must_contain("AdvanceRefused(AdvanceRefusedOutput)", "4");
    signal_witness.must_contain("Error(ErrorOutput)", "4");
    signal_witness.must_contain("Rejected(RejectedOutput)", "4");

    // Nexus exposes internal features as schema-emitted action/effect nouns.
    nexus_witness.must_contain("pub enum CommandSemaWrite {", "4");
    nexus_witness.must_contain("ChangeCertainty(CertaintyChange)", "4");
    nexus_witness.must_contain("BumpImportance(ImportanceBump)", "4");
    nexus_witness.must_contain("RegisterReferent(ReferentRegistration)", "4");
    nexus_witness.must_contain("pub enum NexusEffectCommand {", "4");
    nexus_witness.must_contain("ClassifyState(Statement)", "4");
    nexus_witness.must_contain("ClassifyState(Statement)", "4");
    nexus_witness.must_contain("RecordWithImpliedReferents(RecordRequest)", "4");
    nexus_witness.must_contain("GuardRecord(RecordRequest)", "4");
    nexus_witness.must_contain("ProposeWithImpliedReferents(Proposal)", "4");
    nexus_witness.must_contain("Propose(Proposal)", "4");
    nexus_witness.must_contain("SupersedeWithImpliedReferents(Supersession)", "4");
    nexus_witness.must_not_contain("pub struct GuardRemove(Removal);", "4");
    nexus_witness.must_not_contain("pub struct CollectRemovalCandidates(", "4");
    nexus_witness.must_contain("ChangeRecordWithImpliedReferents(RecordChange)", "4");
    nexus_witness.must_contain("GuardChangeRecord(RecordChange)", "4");
    nexus_witness.must_contain("GuardReferentRegistration(ReferentRegistration)", "4");
    nexus_witness.must_contain("StateClassified(RecordRequest)", "4");
    nexus_witness.must_contain("StateClassified(RecordRequest)", "4");
    nexus_witness.must_contain("ReferentGuardianRejected(ReferentGuardianRejection)", "4");

    // The split SEMA module carries plane-local roots and imported payload nouns.
    sema_witness.must_contain("pub enum WriteInput {", "4");
    sema_witness.must_contain("Record(Entry)", "4");
    sema_witness.must_not_contain("Remove(Remove)", "4");
    sema_witness.must_contain("ChangeCertainty(CertaintyChange)", "4");
    sema_witness.must_contain("BumpImportance(ImportanceBump)", "4");
    sema_witness.must_contain("RegisterReferent(ReferentRegistration)", "4");
    sema_witness.must_contain("pub enum ReadInput {", "4");
    sema_witness.must_contain("Observe(Query)", "4");
    sema_witness.must_contain("PublicIntent(DomainScopes)", "4");
    sema_witness.must_contain("PublicTextSearch(SearchText)", "4");
    sema_witness.must_contain("Lookup(RecordIdentifier)", "4");
    sema_witness.must_contain("Count(Query)", "4");
    sema_witness.must_contain("PublicIntentResults(ObservedRecords)", "4");
    sema_witness.must_contain("Recorded(SemaReceipt)", "4");
    sema_witness.must_contain("CertaintyChanged(CertaintyChangeReceipt)", "4");
    sema_witness.must_contain("ImportanceBumped(ImportanceBumpReceipt)", "4");
    sema_witness.must_contain("ReferentRegistered(ReferentRegistrationReceipt)", "4");

    // The schema-emitted unit enums carry bare variants.
    signal_witness.must_contain("pub enum ValidationError", "4");
    signal_witness.must_contain("EmptyDomain", "4");
    signal_witness.must_contain("EmptyDescription", "4");
    signal_witness.must_contain("EmptyQueryDomain", "4");
    signal_witness.must_contain("EmptyKeyword", "4");
    signal_witness.must_contain("EmptySearchText", "4");
    signal_witness.must_contain("EmptyQueryReferent", "4");
}
