//! Architectural-truth witnesses for the closed claims in operator 271
//! `reports/operator/271-context-maintenance-current-state-2026-06-01.md`.
//!
//! Coverage in this file:
//! - Claim 4 — strict schema syntax and honest enum bodies CLOSED.
//!   The production-path schema `schema/lib.schema` carries parenthesized
//!   data-variant records `(Record Entry) (Observe Query)` and bare
//!   PascalCase unit variants; the retired `Record@Entry` short-suffix
//!   sugar is absent. The companion `schema/lib.asschema` artifact
//!   carries the lifted honest shape.
//!
//! Spirit-next is the production-pilot consumer of schema-emitted nouns,
//! so its source schema is the production witness for claim 4: the schema
//! the daemon and CLI binaries actually use must read as honest variants.
//!
//! Behavioural witnesses for the schema-emitted plane chain live in
//! `tests/generated_signal_plane.rs` and `tests/runtime_triad.rs`.

const LIB_SCHEMA: &str = include_str!("../schema/lib.schema");
const LIB_ASSCHEMA: &str = include_str!("../schema/lib.asschema");

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
}

/// Claim 4 — `schema/lib.schema` declares the `Input` enum body with
/// honest parenthesized data variants. The retired `Record@Entry`
/// short-suffix sugar is absent from the active production schema.
#[test]
fn lib_schema_input_uses_honest_parenthesized_data_variants() {
    let witness = SchemaSourceWitness::new("schema/lib.schema", LIB_SCHEMA);

    // The active production Input enum body — honest data variants.
    witness.must_contain(
        "[(Record Entry) (Observe Query) (Lookup RecordIdentifier) (Count Query) (Remove RecordIdentifier)]",
        "4",
    );

    // Retired short-suffix shorthand must not appear.
    witness.must_not_contain("Record@Entry", "4");
    witness.must_not_contain("Observe@Query", "4");
    witness.must_not_contain("Remove@RecordIdentifier", "4");
    witness.must_not_contain("@Vec", "4");
    witness.must_not_contain("@Option", "4");
    witness.must_not_contain("@KeyValue", "4");
    witness.must_not_contain("@Map", "4");
}

/// Claim 4 — `schema/lib.schema` declares the `Output` enum body with
/// honest parenthesized data variants.
#[test]
fn lib_schema_output_uses_honest_parenthesized_data_variants() {
    let witness = SchemaSourceWitness::new("schema/lib.schema", LIB_SCHEMA);

    // The active production Output enum body.
    witness.must_contain("(RecordAccepted SemaReceipt)", "4");
    witness.must_contain("(RecordsObserved ObservedRecords)", "4");
    witness.must_contain("(RecordFound FoundRecord)", "4");
    witness.must_contain("(RecordsCounted CountedRecords)", "4");
    witness.must_contain("(RecordRemoved RemoveReceipt)", "4");
    witness.must_contain("(Error ErrorReport)", "4");
    witness.must_contain("(Rejected SignalRejection)", "4");
}

/// Claim 4 — `schema/lib.schema` declares the `ValidationError` enum body
/// with bare PascalCase unit variants. This is the honest form for
/// payload-free variants; no parens, no sigil.
#[test]
fn lib_schema_unit_variant_enum_uses_bare_pascal_case_atoms() {
    let witness = SchemaSourceWitness::new("schema/lib.schema", LIB_SCHEMA);

    // ValidationError carries three bare unit variants.
    witness.must_contain(
        "ValidationError [EmptyTopic EmptyDescription EmptyQueryTopic]",
        "4",
    );

    // Kind is a five-variant unit enum.
    witness.must_contain(
        "Kind [Decision Principle Correction Clarification Constraint]",
        "4",
    );

    // Magnitude is a seven-variant unit enum.
    witness.must_contain(
        "Magnitude [Minimum VeryLow Low Medium High VeryHigh Maximum]",
        "4",
    );
}

/// Claim 4 — The whole production schema MUST be free of the retired `@`
/// sigil. This is the broader regression sweep — even a single `@`
/// character anywhere in the schema would resurrect the rejected shorthand.
#[test]
fn lib_schema_carries_no_at_sigil_anywhere() {
    let witness = SchemaSourceWitness::new("schema/lib.schema", LIB_SCHEMA);
    witness.must_not_contain("@", "4");
}

/// Claim 4 — The lifted `lib.asschema` artifact carries the assembled
/// shape with the parenthesized variants lifted into typed
/// `(VariantName (Some (Plain TypeName)))` records — the assembled-data
/// equivalent of honest source-level enum bodies.
#[test]
fn lib_asschema_lifts_honest_data_variants_into_typed_records() {
    let witness = SchemaSourceWitness::new("schema/lib.asschema", LIB_ASSCHEMA);

    // Input variants lifted to the assembled (Some (Plain ...)) form.
    witness.must_contain("(Record (Some (Plain Entry)))", "4");
    witness.must_contain("(Observe (Some (Plain Query)))", "4");
    witness.must_contain("(Lookup (Some (Plain RecordIdentifier)))", "4");
    witness.must_contain("(Count (Some (Plain Query)))", "4");
    witness.must_contain("(Remove (Some (Plain RecordIdentifier)))", "4");

    // Output variants similarly lifted.
    witness.must_contain("(RecordAccepted (Some (Plain SemaReceipt)))", "4");
    witness.must_contain("(RecordFound (Some (Plain FoundRecord)))", "4");
    witness.must_contain("(RecordsCounted (Some (Plain CountedRecords)))", "4");
    witness.must_contain("(RecordRemoved (Some (Plain RemoveReceipt)))", "4");

    // Unit variants land as `(VariantName None)`.
    witness.must_contain("(EmptyTopic None)", "4");
    witness.must_contain("(Decision None)", "4");
    witness.must_contain("(Minimum None)", "4");

    // No retired sigil leaks into the artifact either.
    witness.must_not_contain("@", "4");
}

/// Claim 4 — The schema-emitted Rust source surface mirrors the honest
/// schema. `src/schema/lib.rs` is the generated module the spirit-next
/// crate compiles against; it MUST declare the same enums with the same
/// variants. This is the projection-side witness for claim 4.
#[test]
fn schema_emitted_rust_module_mirrors_honest_enum_variants() {
    let emitted_rust = include_str!("../src/schema/lib.rs");
    let witness = SchemaSourceWitness::new("src/schema/lib.rs", emitted_rust);

    // The schema-emitted Input enum carries the same data variants.
    witness.must_contain("pub enum Input {", "4");
    witness.must_contain("Record(Entry)", "4");
    witness.must_contain("Observe(Query)", "4");
    witness.must_contain("Lookup(RecordIdentifier)", "4");
    witness.must_contain("Count(Query)", "4");
    witness.must_contain("Remove(RecordIdentifier)", "4");

    // The schema-emitted Output enum carries the same data variants.
    witness.must_contain("pub enum Output {", "4");
    witness.must_contain("RecordAccepted(SemaReceipt)", "4");
    witness.must_contain("RecordsObserved(ObservedRecords)", "4");
    witness.must_contain("RecordFound(FoundRecord)", "4");
    witness.must_contain("RecordsCounted(CountedRecords)", "4");
    witness.must_contain("RecordRemoved(RemoveReceipt)", "4");
    witness.must_contain("Error(ErrorReport)", "4");
    witness.must_contain("Rejected(SignalRejection)", "4");

    // The schema-emitted unit enums carry bare variants.
    witness.must_contain("pub enum ValidationError", "4");
    witness.must_contain("EmptyTopic", "4");
    witness.must_contain("EmptyDescription", "4");
    witness.must_contain("EmptyQueryTopic", "4");
}
