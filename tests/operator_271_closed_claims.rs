//! Architectural-truth witnesses for the closed claims in operator 271
//! `reports/operator/271-context-maintenance-current-state-2026-06-01.md`.
//!
//! Coverage in this file:
//! - Claim 4 — strict schema syntax and honest enum bodies CLOSED.
//!   The production-path plane schemas carry compact root-header object
//!   names (`Record Observe Lookup ...`, `WriteInput ReadInput`) and define
//!   those exported objects in the namespace (`Record Entry`, `Observe
//!   Query`, ...). The retired `Record@Entry` short-suffix sugar is absent.
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
    witness.must_contain("[Record Observe Lookup Count Remove LookupStash]", "4");
    witness.must_contain("Record Entry", "4");
    witness.must_contain("Observe Query", "4");
    witness.must_contain("Lookup RecordIdentifier", "4");
    witness.must_contain("Count Query", "4");
    witness.must_contain("Remove RecordIdentifier", "4");
    witness.must_contain("LookupStash StashHandle", "4");

    // Retired short-suffix shorthand must not appear.
    witness.must_not_contain("Record@Entry", "4");
    witness.must_not_contain("Observe@Query", "4");
    witness.must_not_contain("Remove@RecordIdentifier", "4");
    witness.must_not_contain("@Vec", "4");
    witness.must_not_contain("@Option", "4");
    witness.must_not_contain("@KeyValue", "4");
    witness.must_not_contain("@Map", "4");
}

/// Claim 4 — `schema/signal.schema` declares the `Output` enum body with
/// compact exported object names and namespace declarations.
#[test]
fn signal_schema_output_uses_exported_object_variant_names() {
    let witness = SchemaSourceWitness::new("schema/signal.schema", SIGNAL_SCHEMA);

    // The active production Output enum body.
    witness.must_contain(
        "[RecordAccepted RecordsObserved RecordsStashed RecordFound RecordsCounted RecordRemoved Error Rejected]",
        "4",
    );
    witness.must_contain("RecordAccepted SemaReceipt", "4");
    witness.must_contain("RecordsObserved ObservedRecords", "4");
    witness.must_contain("RecordFound FoundRecord", "4");
    witness.must_contain("RecordsCounted CountedRecords", "4");
    witness.must_contain("RecordRemoved RemoveReceipt", "4");
    witness.must_contain("Error ErrorReport", "4");
    witness.must_contain("Rejected SignalRejection", "4");
}

/// Claim 4 — `schema/signal.schema` declares the `ValidationError` enum body
/// with bare PascalCase unit variants. This is the honest form for
/// payload-free variants; no parens, no sigil.
#[test]
fn signal_schema_unit_variant_enum_uses_bare_pascal_case_atoms() {
    let witness = SchemaSourceWitness::new("schema/signal.schema", SIGNAL_SCHEMA);

    // ValidationError carries four bare unit variants per designer 480 —
    // StashHandleNotFound joins the Stash effect's lookup-by-handle path.
    witness.must_contain(
        "ValidationError [EmptyTopic EmptyDescription EmptyQueryTopic StashHandleNotFound]",
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
    let nexus_witness = SchemaSourceWitness::new("schema/nexus.schema", NEXUS_SCHEMA);
    let sema_witness = SchemaSourceWitness::new("schema/sema.schema", SEMA_SCHEMA);

    signal_witness.must_not_contain("@", "4");
    nexus_witness.must_not_contain("@", "4");
    sema_witness.must_not_contain("@", "4");
}

/// Claim 4 — The authored schemas are the durable schema values. Each
/// one decodes into `SchemaSource` and archives through rkyv without an
/// assembled checked-in artifact between source and emitted Rust.
#[test]
fn split_schema_sources_decode_and_archive_as_typed_schema_values() {
    let signal_witness = SchemaSourceWitness::new("schema/signal.schema", SIGNAL_SCHEMA);
    let nexus_witness = SchemaSourceWitness::new("schema/nexus.schema", NEXUS_SCHEMA);
    let sema_witness = SchemaSourceWitness::new("schema/sema.schema", SEMA_SCHEMA);

    signal_witness.must_round_trip_as_schema_source();
    nexus_witness.must_round_trip_as_schema_source();
    sema_witness.must_round_trip_as_schema_source();

    signal_witness.must_contain("[Record Observe Lookup Count Remove LookupStash]", "4");
    signal_witness.must_contain("Record Entry", "4");
    signal_witness.must_contain("Observe Query", "4");
    signal_witness.must_contain("Lookup RecordIdentifier", "4");
    sema_witness.must_contain("[WriteInput ReadInput]", "4");
    sema_witness.must_contain("WriteInput [Record Remove]", "4");
    sema_witness.must_contain("Recorded SemaReceipt", "4");
    nexus_witness.must_contain("CommandSemaWrite SemaWriteInput", "4");
}

/// Claim 4 — The schema-emitted Rust source surface mirrors the honest
/// schema. The generated plane modules the spirit crate compiles against
/// MUST declare the same enums with the same variants. This is the
/// projection-side witness for claim 4.
#[test]
fn schema_emitted_rust_modules_mirror_honest_enum_variants() {
    let signal_rust = include_str!("../src/schema/signal.rs");
    let sema_rust = include_str!("../src/schema/sema.rs");
    let signal_witness = SchemaSourceWitness::new("src/schema/signal.rs", signal_rust);
    let sema_witness = SchemaSourceWitness::new("src/schema/sema.rs", sema_rust);

    // The schema-emitted Input enum carries exported alias nouns.
    signal_witness.must_contain("pub enum Input {", "4");
    signal_witness.must_contain("pub type Record = Entry;", "4");
    signal_witness.must_contain("pub type Observe = Query;", "4");
    signal_witness.must_contain("pub type Lookup = RecordIdentifier;", "4");
    signal_witness.must_contain("Record(Record)", "4");
    signal_witness.must_contain("Observe(Observe)", "4");
    signal_witness.must_contain("Lookup(Lookup)", "4");
    signal_witness.must_contain("Count(Count)", "4");
    signal_witness.must_contain("Remove(Remove)", "4");

    // The schema-emitted Output enum carries exported alias nouns.
    signal_witness.must_contain("pub enum Output {", "4");
    signal_witness.must_contain("pub type RecordAccepted = SemaReceipt;", "4");
    signal_witness.must_contain("pub type RecordsObserved = ObservedRecords;", "4");
    signal_witness.must_contain("pub type RecordsStashed = StashedObservation;", "4");
    signal_witness.must_contain("RecordAccepted(RecordAccepted)", "4");
    signal_witness.must_contain("RecordsObserved(RecordsObserved)", "4");
    signal_witness.must_contain("RecordsStashed(RecordsStashed)", "4");
    signal_witness.must_contain("RecordFound(RecordFound)", "4");
    signal_witness.must_contain("RecordsCounted(RecordsCounted)", "4");
    signal_witness.must_contain("RecordRemoved(RecordRemoved)", "4");
    signal_witness.must_contain("Error(Error)", "4");
    signal_witness.must_contain("Rejected(Rejected)", "4");

    // The split SEMA module carries plane-local roots and imported payload nouns.
    sema_witness.must_contain("pub enum WriteInput {", "4");
    sema_witness.must_contain("Record(Record)", "4");
    sema_witness.must_contain("Remove(Remove)", "4");
    sema_witness.must_contain("pub enum ReadInput {", "4");
    sema_witness.must_contain("Observe(Observe)", "4");
    sema_witness.must_contain("Lookup(Lookup)", "4");
    sema_witness.must_contain("Count(Count)", "4");
    sema_witness.must_contain("pub type Recorded = SemaReceipt;", "4");

    // The schema-emitted unit enums carry bare variants.
    signal_witness.must_contain("pub enum ValidationError", "4");
    signal_witness.must_contain("EmptyTopic", "4");
    signal_witness.must_contain("EmptyDescription", "4");
    signal_witness.must_contain("EmptyQueryTopic", "4");
}
