//! Architectural-truth witnesses for the closed claims in operator 271
//! `reports/operator/271-context-maintenance-current-state-2026-06-01.md`.
//!
//! Coverage in this file:
//! - Claim 4 — strict schema syntax and honest enum bodies CLOSED.
//!   The production-path plane schemas carry compact root-header object
//!   names (`Record Observe Lookup ...`, `WriteInput ReadInput`) and define
//!   those exported objects in the namespace (`Record RecordRequest`, `Observe
//!   Query`, ...). Namespace enums that carry same-named payloads use
//!   self-tagged signatures (`(Record)`). The retired `Record@Entry`
//!   short-suffix sugar is absent.
//!   The authored schema source decodes into a typed `SchemaSource` value,
//!   round-trips through rkyv, and the emitted Rust carries the alias shape.
//!
//! Spirit-next is the production-pilot consumer of schema-emitted nouns,
//! so its source schema is the production witness for claim 4: the schema
//! the daemon and CLI binaries actually use must read as honest variants.
//!
//! Behavioural witnesses for the schema-emitted plane chain live in
//! `tests/generated_signal_plane.rs` and `tests/runtime_triad.rs`.

const SIGNAL_SCHEMA: &str = include_str!("../schema/signal.schema");
const DOMAIN_SCHEMA: &str = include_str!("../schema/domain.schema");
const NEXUS_SCHEMA: &str = include_str!("../schema/nexus.schema");
const SEMA_SCHEMA: &str = include_str!("../schema/sema.schema");

use schema_next::SchemaSourceArtifact;

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
}

/// Claim 4 — `schema/signal.schema` declares the `Input` enum body with
/// compact exported object names. The retired `Record@Entry`
/// short-suffix sugar is absent from the active production schema, and
/// the payload shape lives in namespace declarations.
#[test]
fn signal_schema_input_uses_exported_object_variant_names() {
    let witness = SchemaSourceWitness::new("schema/signal.schema", SIGNAL_SCHEMA);

    // The active production Input enum body — compact exported objects.
    witness.must_contain(
        "[State Record Propose Clarify Supersede Retire Observe PublicRecords PrivateRecords Lookup Count Remove ChangeCertainty BumpImportance ChangeRecord RegisterReferent LookupStash CollectRemovalCandidates Tap Untap (SubscribeIntent SubscribeIntent opens IntentEventStream) Version Marker]",
        "4",
    );
    witness.must_contain("State Statement", "4");
    witness.must_contain("Record RecordRequest", "4");
    witness.must_contain("Propose Proposal", "4");
    witness.must_contain("Clarify Clarification", "4");
    witness.must_contain("Supersede Supersession", "4");
    witness.must_contain("Retire Retirement", "4");
    witness.must_contain("Observe Query", "4");
    witness.must_contain("PublicRecords RecordSelection", "4");
    witness.must_contain("PrivateRecords RecordSelection", "4");
    witness.must_contain("Lookup RecordIdentifier", "4");
    witness.must_contain("Count Query", "4");
    witness.must_contain("Remove Removal", "4");
    witness.must_contain("ChangeCertainty CertaintyChange", "4");
    witness.must_contain("BumpImportance ImportanceBump", "4");
    witness.must_contain("ChangeRecord RecordChange", "4");
    witness.must_contain("RegisterReferent ReferentRegistration", "4");
    witness.must_contain(
        "Justification { Testimony * Reasoning * }",
        "4",
    );
    witness.must_contain("RecordRequest { Entry * Justification * }", "4");
    witness.must_contain("Proposal { Entry * Justification * }", "4");
    witness.must_contain("LookupStash StashHandle", "4");
    witness.must_contain("SubscribeIntent Query", "4");
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
    let witness = SchemaSourceWitness::new("schema/domain.schema", DOMAIN_SCHEMA);

    witness.must_contain(
        "(Craft [Electronics Construction Carpentry Metalworking Sewing Manufacturing Repair Engineering Handicraft Invention])",
        "software-domain",
    );
    witness.must_contain("(Software [", "software-domain");
    witness.must_contain("(Technology [", "software-domain");
    witness.must_contain("(Hardware [Energy Power Automation Robotics Networking Materials Machinery Instrumentation Aerospace])", "software-domain");
    witness.must_contain(
        "(Languages [ProgrammingLanguages ProgrammingParadigms TypeSystems Compilation Interpretation Parsing LexicalAnalysis Grammars CodeGeneration Metaprogramming Macros DomainSpecificLanguages RuntimeEnvironments GarbageCollection MemoryManagement ForeignFunctionInterfaces])",
        "software-domain",
    );
    witness.must_contain(
        "(Quality [Testing UnitTesting IntegrationTesting EndToEndTesting PropertyBasedTesting Fuzzing TestAutomation Mocking CodeCoverage Debugging Profiling Benchmarking PerformanceOptimization LoadTesting CodeReview Refactoring Linting Formatting TechnicalDebt])",
        "software-domain",
    );
    witness.must_contain(
        "(Operations [ContinuousIntegration ContinuousDelivery BuildSystem ReleaseEngineering DependencyManagement PackageManagement ArtifactManagement Deployment Provisioning InfrastructureAsCode Orchestration ConfigurationManagement AutoScaling CapacityPlanning SiteReliability IncidentResponse DisasterRecovery RateLimiting])",
        "software-domain",
    );
    witness.must_contain(
        "(Engineering [SoftwareArchitecture SoftwareDesign DesignPatterns DomainDrivenDesign ApplicationProgrammingInterfaces Microservices Serverless CloudComputing EdgeComputing Scalability Reliability Maintainability Portability Interoperability Modularity Abstraction RequirementsEngineering Documentation VersionControl SoftwareDevelopmentProcess SoftwareMaintenance SoftwareEngineeringManagement])",
        "software-domain",
    );
    witness.must_not_contain(
        "(Craft [Programming Architecture Schema Infrastructure Versioning Testing",
        "software-domain",
    );
    witness.must_contain(
        "(Equivalence [(Technology Hardware Networking) (Technology Software Distributed Networking)])",
        "software-domain",
    );
}

/// Claim 4 — `schema/signal.schema` declares the `Output` enum body with
/// compact exported object names and namespace declarations.
#[test]
fn signal_schema_output_uses_exported_object_variant_names() {
    let witness = SchemaSourceWitness::new("schema/signal.schema", SIGNAL_SCHEMA);

    // The active production Output enum body.
    witness.must_contain(
        "[RecordAccepted Proposed Clarified Superseded Retired GuardianRejected ReferentGuardianRejected RecordsObserved RecordsStashed RecordFound RecordsCounted RecordRemoved CertaintyChanged ImportanceBumped RecordChanged ReferentRegistered RemovalCandidatesCollected ObservationTapped ObservationUntapped SubscriptionStarted VersionReported MarkerReported (Event IntentEvent) Error Rejected]",
        "4",
    );
    witness.must_contain("RecordAccepted RecordIdentifier", "4");
    witness.must_contain("Proposed RecordIdentifier", "4");
    witness.must_contain("Clarified ClarificationReceipt", "4");
    witness.must_contain("Superseded SupersessionReceipt", "4");
    witness.must_contain("Retired RetirementReceipt", "4");
    witness.must_contain("GuardianRejected GuardianRejection", "4");
    witness.must_contain("ReferentGuardianRejected ReferentGuardianRejection", "4");
    witness.must_contain("RecordsObserved ObservedRecords", "4");
    witness.must_contain("RecordFound FoundRecord", "4");
    witness.must_contain("RecordsCounted CountedRecords", "4");
    witness.must_contain("RecordRemoved RemoveReceipt", "4");
    witness.must_contain("CertaintyChanged CertaintyChangeReceipt", "4");
    witness.must_contain("ImportanceBumped ImportanceBumpReceipt", "4");
    witness.must_contain("RecordChanged RecordChangeReceipt", "4");
    witness.must_contain("ReferentRegistered ReferentRegistrationReceipt", "4");
    witness.must_contain("SubscriptionStarted IntentSubscription", "4");
    witness.must_contain("VersionReported VersionReport", "4");
    witness.must_contain("MarkerReported DatabaseMarker", "4");
    witness.must_contain("VersionReport { VersionText * }", "4");
    witness.must_contain(
        "IntentEvent [(IntentRecorded IntentRecorded belongs IntentEventStream) (IntentClarified IntentClarified belongs IntentEventStream) (IntentSuperseded IntentSuperseded belongs IntentEventStream) (IntentRetired IntentRetired belongs IntentEventStream)]",
        "4",
    );
    witness.must_contain("Error ErrorReport", "4");
    witness.must_contain("Rejected SignalRejection", "4");
}

/// Claim 4 — `schema/signal.schema` declares the `ValidationError` enum body
/// with bare PascalCase unit variants. This is the honest form for
/// payload-free variants; no parens, no sigil.
#[test]
fn signal_schema_unit_variant_enum_uses_bare_pascal_case_atoms() {
    let witness = SchemaSourceWitness::new("schema/signal.schema", SIGNAL_SCHEMA);

    // ValidationError carries bare unit variants per designer 480; keyword
    // and text-query validation add typed read-predicate failures.
    witness.must_contain(
        "ValidationError [EmptyDomain EmptyDescription EmptyQueryDomain EmptyKeyword EmptySearchText EmptyQueryReferent StashHandleNotFound]",
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
    let signal_witness = SchemaSourceWitness::new("schema/signal.schema", SIGNAL_SCHEMA);
    let domain_witness = SchemaSourceWitness::new("schema/domain.schema", DOMAIN_SCHEMA);
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
    let signal_witness = SchemaSourceWitness::new("schema/signal.schema", SIGNAL_SCHEMA);
    let domain_witness = SchemaSourceWitness::new("schema/domain.schema", DOMAIN_SCHEMA);
    let nexus_witness = SchemaSourceWitness::new("schema/nexus.schema", NEXUS_SCHEMA);
    let sema_witness = SchemaSourceWitness::new("schema/sema.schema", SEMA_SCHEMA);

    signal_witness.must_round_trip_as_schema_source();
    domain_witness.must_round_trip_as_schema_source();
    nexus_witness.must_round_trip_as_schema_source();
    sema_witness.must_round_trip_as_schema_source();

    signal_witness.must_contain(
        "[State Record Propose Clarify Supersede Retire Observe PublicRecords PrivateRecords Lookup Count Remove ChangeCertainty BumpImportance ChangeRecord RegisterReferent LookupStash CollectRemovalCandidates Tap Untap (SubscribeIntent SubscribeIntent opens IntentEventStream) Version Marker]",
        "4",
    );
    signal_witness.must_contain("State Statement", "4");
    signal_witness.must_contain("Record RecordRequest", "4");
    signal_witness.must_contain("Observe Query", "4");
    signal_witness.must_contain("PublicRecords RecordSelection", "4");
    signal_witness.must_contain("PrivateRecords RecordSelection", "4");
    signal_witness.must_contain("Lookup RecordIdentifier", "4");
    signal_witness.must_contain("ChangeCertainty CertaintyChange", "4");
    signal_witness.must_contain("BumpImportance ImportanceBump", "4");
    signal_witness.must_contain("ChangeRecord RecordChange", "4");
    signal_witness.must_contain("RegisterReferent ReferentRegistration", "4");
    signal_witness.must_contain("SubscribeIntent Query", "4");
    signal_witness.must_contain("Version", "4");
    signal_witness.must_contain("Marker", "4");
    sema_witness.must_contain("[WriteInput ReadInput]", "4");
    sema_witness.must_contain(
        "WriteInput [(Record) (Remove) (ChangeCertainty) (BumpImportance) (ChangeRecord) (RegisterReferent)]",
        "4",
    );
    sema_witness.must_contain("Recorded SemaReceipt", "4");
    sema_witness.must_contain(
        "WriteOutput [(Recorded) (Removed) (CertaintyChanged) (ImportanceBumped) (RecordChanged) (ReferentRegistered) (Missed)]",
        "4",
    );
    nexus_witness.must_contain(
        "CommandSemaWrite [(Record) (Remove) (ChangeCertainty) (BumpImportance) (ChangeRecord) (RegisterReferent)]",
        "4",
    );
    nexus_witness.must_contain("NexusAction [(CommandSemaWrite)", "4");
    nexus_witness.must_contain(
        "NexusEffectCommand [(Stash) (ClassifyState) (RecordWithImpliedReferents) (GuardRecord) (ProposeWithImpliedReferents) (Propose) (Clarify) (SupersedeWithImpliedReferents) (Supersede) (Retire) (GuardRemove) (ChangeRecordWithImpliedReferents) (GuardChangeRecord) (GuardReferentRegistration) (OpenIntentSubscription) (CollectRemovalCandidates) (OpenObserverTap) (CloseObserverTap)]",
        "4",
    );
    nexus_witness.must_contain("ClassifyState Statement", "4");
    nexus_witness.must_contain("RecordWithImpliedReferents RecordRequest", "4");
    nexus_witness.must_contain("GuardRecord RecordRequest", "4");
    nexus_witness.must_contain("ProposeWithImpliedReferents Proposal", "4");
    nexus_witness.must_contain("Propose Proposal", "4");
    nexus_witness.must_contain("Clarify Clarification", "4");
    nexus_witness.must_contain("SupersedeWithImpliedReferents Supersession", "4");
    nexus_witness.must_contain("Supersede Supersession", "4");
    nexus_witness.must_contain("Retire Retirement", "4");
    nexus_witness.must_contain("GuardRemove Removal", "4");
    nexus_witness.must_contain("ChangeRecordWithImpliedReferents RecordChange", "4");
    nexus_witness.must_contain("GuardChangeRecord RecordChange", "4");
    nexus_witness.must_contain("GuardReferentRegistration ReferentRegistration", "4");
    nexus_witness.must_contain("OpenIntentSubscription Query", "4");
    nexus_witness.must_contain(
        "NexusEffectResult [(Stashed) (StateClassified) (RecordReferentsSettled) (ProposeReferentsSettled) (SupersedeReferentsSettled) (ChangeRecordReferentsSettled) (Recorded) (Proposed) (Clarified) (Superseded) (Retired) (Removed) (RecordChanged) (GuardianRejected) (ReferentRegistered) (ReferentGuardianRejected) (OperationFailed) (IntentSubscriptionOpened) (RemovalCandidatesCollected) (ObserverTapOpened) (ObserverTapClosed)]",
        "4",
    );
    nexus_witness.must_contain("StateClassified RecordRequest", "4");
    nexus_witness.must_contain("RecordReferentsSettled RecordRequest", "4");
    nexus_witness.must_contain("ProposeReferentsSettled Proposal", "4");
    nexus_witness.must_contain("SupersedeReferentsSettled Supersession", "4");
    nexus_witness.must_contain("ChangeRecordReferentsSettled RecordChange", "4");
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
    let signal_rust = include_str!("../src/schema/signal.rs");
    let nexus_rust = include_str!("../src/schema/nexus.rs");
    let sema_rust = include_str!("../src/schema/sema.rs");
    let signal_witness = SchemaSourceWitness::new("src/schema/signal.rs", signal_rust);
    let nexus_witness = SchemaSourceWitness::new("src/schema/nexus.rs", nexus_rust);
    let sema_witness = SchemaSourceWitness::new("src/schema/sema.rs", sema_rust);

    // The schema-emitted Input enum carries exported wrapper nouns.
    signal_witness.must_contain("pub enum Input {", "4");
    signal_witness.must_contain("pub struct State(Statement);", "4");
    signal_witness.must_contain("State(State)", "4");
    signal_witness.must_contain("pub struct Record(RecordRequest);", "4");
    signal_witness.must_contain("pub struct Propose(Proposal);", "4");
    signal_witness.must_contain("pub struct Observe(Query);", "4");
    signal_witness.must_contain("pub struct PublicRecords(RecordSelection);", "4");
    signal_witness.must_contain("pub struct PrivateRecords(RecordSelection);", "4");
    signal_witness.must_contain("pub struct Lookup(RecordIdentifier);", "4");
    signal_witness.must_contain("Record(Record)", "4");
    signal_witness.must_contain("Observe(Observe)", "4");
    signal_witness.must_contain("PublicRecords(PublicRecords)", "4");
    signal_witness.must_contain("PrivateRecords(PrivateRecords)", "4");
    signal_witness.must_contain("Lookup(Lookup)", "4");
    signal_witness.must_contain("Count(Count)", "4");
    signal_witness.must_contain("Remove(Remove)", "4");
    signal_witness.must_contain("ChangeCertainty(ChangeCertainty)", "4");
    signal_witness.must_contain("BumpImportance(BumpImportance)", "4");
    signal_witness.must_contain("RegisterReferent(RegisterReferent)", "4");
    signal_witness.must_contain("Version", "4");
    signal_witness.must_contain("Marker", "4");

    // The schema-emitted Output enum carries exported wrapper nouns.
    signal_witness.must_contain("pub enum Output {", "4");
    signal_witness.must_contain("pub struct RecordAccepted(RecordIdentifier);", "4");
    signal_witness.must_contain("pub struct RecordsObserved(ObservedRecords);", "4");
    signal_witness.must_contain("pub struct RecordsStashed(StashedObservation);", "4");
    signal_witness.must_contain("RecordAccepted(RecordAccepted)", "4");
    signal_witness.must_contain("RecordsObserved(RecordsObserved)", "4");
    signal_witness.must_contain("RecordsStashed(RecordsStashed)", "4");
    signal_witness.must_contain("RecordFound(RecordFound)", "4");
    signal_witness.must_contain("RecordsCounted(RecordsCounted)", "4");
    signal_witness.must_contain("RecordRemoved(RecordRemoved)", "4");
    signal_witness.must_contain("CertaintyChanged(CertaintyChanged)", "4");
    signal_witness.must_contain("ImportanceBumped(ImportanceBumped)", "4");
    signal_witness.must_contain("ReferentRegistered(ReferentRegistered)", "4");
    signal_witness.must_contain("ReferentGuardianRejected(ReferentGuardianRejected)", "4");
    signal_witness.must_contain("VersionReported(VersionReported)", "4");
    signal_witness.must_contain("MarkerReported(MarkerReported)", "4");
    signal_witness.must_contain("Error(Error)", "4");
    signal_witness.must_contain("Rejected(Rejected)", "4");

    // Nexus exposes internal features as schema-emitted action/effect nouns.
    nexus_witness.must_contain("pub enum CommandSemaWrite {", "4");
    nexus_witness.must_contain("ChangeCertainty(ChangeCertainty)", "4");
    nexus_witness.must_contain("BumpImportance(BumpImportance)", "4");
    nexus_witness.must_contain("RegisterReferent(RegisterReferent)", "4");
    nexus_witness.must_contain("pub enum NexusEffectCommand {", "4");
    nexus_witness.must_contain("pub struct ClassifyState(Statement);", "4");
    nexus_witness.must_contain("ClassifyState(ClassifyState)", "4");
    nexus_witness.must_contain("pub struct RecordWithImpliedReferents(RecordRequest);", "4");
    nexus_witness.must_contain("pub struct GuardRecord(RecordRequest);", "4");
    nexus_witness.must_contain("pub struct ProposeWithImpliedReferents(Proposal);", "4");
    nexus_witness.must_contain("pub struct Propose(Proposal);", "4");
    nexus_witness.must_contain(
        "pub struct SupersedeWithImpliedReferents(Supersession);",
        "4",
    );
    nexus_witness.must_contain("pub struct GuardRemove(Removal);", "4");
    nexus_witness.must_contain(
        "pub struct ChangeRecordWithImpliedReferents(RecordChange);",
        "4",
    );
    nexus_witness.must_contain("pub struct GuardChangeRecord(RecordChange);", "4");
    nexus_witness.must_contain(
        "pub struct GuardReferentRegistration(ReferentRegistration);",
        "4",
    );
    nexus_witness.must_contain("pub struct StateClassified(RecordRequest);", "4");
    nexus_witness.must_contain("StateClassified(StateClassified)", "4");
    nexus_witness.must_contain("ReferentGuardianRejected(ReferentGuardianRejected)", "4");

    // The split SEMA module carries plane-local roots and imported payload nouns.
    sema_witness.must_contain("pub enum WriteInput {", "4");
    sema_witness.must_contain("Record(Record)", "4");
    sema_witness.must_contain("Remove(Remove)", "4");
    sema_witness.must_contain("ChangeCertainty(ChangeCertainty)", "4");
    sema_witness.must_contain("BumpImportance(BumpImportance)", "4");
    sema_witness.must_contain("RegisterReferent(RegisterReferent)", "4");
    sema_witness.must_contain("pub enum ReadInput {", "4");
    sema_witness.must_contain("Observe(Observe)", "4");
    sema_witness.must_contain("Lookup(Lookup)", "4");
    sema_witness.must_contain("Count(Count)", "4");
    sema_witness.must_contain("pub struct Recorded(SemaReceipt);", "4");
    sema_witness.must_contain("pub struct CertaintyChanged(CertaintyChangeReceipt);", "4");
    sema_witness.must_contain("pub struct ImportanceBumped(ImportanceBumpReceipt);", "4");
    sema_witness.must_contain(
        "pub struct ReferentRegistered(ReferentRegistrationReceipt);",
        "4",
    );

    // The schema-emitted unit enums carry bare variants.
    signal_witness.must_contain("pub enum ValidationError", "4");
    signal_witness.must_contain("EmptyDomain", "4");
    signal_witness.must_contain("EmptyDescription", "4");
    signal_witness.must_contain("EmptyQueryDomain", "4");
    signal_witness.must_contain("EmptyKeyword", "4");
    signal_witness.must_contain("EmptySearchText", "4");
    signal_witness.must_contain("EmptyQueryReferent", "4");
}
