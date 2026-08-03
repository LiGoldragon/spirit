//! Architectural-truth witnesses for the generated v14 plane.

use schema_language::{ImportResolver, SchemaEngine, SchemaIdentity, SchemaSourceArtifact};

const SIGNAL_SCHEMA: &str = signal_spirit::SIGNAL_SCHEMA_SOURCE;
const DOMAIN_SCHEMA: &str = signal_spirit::DOMAIN_SCHEMA_SOURCE;
const NEXUS_SCHEMA: &str = include_str!("../schema/nexus.schema");
const SEMA_SCHEMA: &str = include_str!("../schema/sema.schema");

#[test]
fn revision_2_signal_schema_has_the_uniform_current_roots() {
    for declaration in [
        "Entry { Domains Kind Description Importance }",
        "Observe Query",
        "Intent DomainScopes",
        "TextSearch SearchText",
        "Lookup RecordIdentifier",
        "Count Query",
        "BumpImportance ImportanceBump",
        "ChangeRecord RecordChange",
    ] {
        assert!(SIGNAL_SCHEMA.contains(declaration), "missing {declaration}");
    }
}

#[test]
fn active_schema_sources_have_no_retired_spirit_vocabulary() {
    for source in [SIGNAL_SCHEMA, NEXUS_SCHEMA, SEMA_SCHEMA] {
        for retired in [
            "Certainty",
            "PrivacySelection",
            "ReferentSelection",
            "RegisterReferent",
            "ChangeCertainty",
            "PublicTextSearch",
            "PublicIntent",
            "PublicRecords",
            "PrivateRecords",
            "RemovalCandidate",
        ] {
            assert!(
                !source.contains(retired),
                "active schema contains {retired}"
            );
        }
    }
}

#[test]
fn sema_schema_declares_only_current_families_and_operations() {
    assert!(SEMA_SCHEMA.contains("WriteInput [(Record) (BumpImportance) (ChangeRecord)]"));
    assert!(SEMA_SCHEMA.contains("ReadInput [(Observe) (Intent) (TextSearch) (Lookup) (Count)]"));
    assert!(SEMA_SCHEMA.contains("StoredRecord { RecordIdentifier Entry }"));
    assert!(SEMA_SCHEMA.contains("Migration { SourceSchemaVersion MigratedRecordCount }"));
    assert!(SEMA_SCHEMA.contains("RecordsFamily (Family"));
    assert!(SEMA_SCHEMA.contains("MigrationsFamily (Family"));
}

#[test]
fn nexus_schema_has_only_admission_and_explicit_lifecycle_effects() {
    assert!(NEXUS_SCHEMA.contains("CommandSemaWrite [(Record) (BumpImportance) (ChangeRecord)]"));
    for effect in [
        "ClassifyState Statement",
        "GuardRecord RecordRequest",
        "Propose Proposal",
        "Clarify Clarification",
        "Supersede Supersession",
        "Retire Retirement",
        "GuardChangeRecord RecordChange",
    ] {
        assert!(NEXUS_SCHEMA.contains(effect), "missing effect {effect}");
    }
}

#[test]
fn split_schema_sources_round_trip_and_lower_as_typed_values() {
    for source in [SIGNAL_SCHEMA, DOMAIN_SCHEMA, NEXUS_SCHEMA, SEMA_SCHEMA] {
        let artifact =
            SchemaSourceArtifact::from_schema_text(source).expect("decode schema source");
        let binary = artifact.to_binary_bytes().expect("archive schema source");
        assert_eq!(
            SchemaSourceArtifact::from_binary_bytes(&binary).expect("recover schema source"),
            artifact
        );
    }

    let resolver = ImportResolver::new()
        .with_module_source("signal-domain", "domain", "0.1.0", DOMAIN_SCHEMA)
        .with_module_source("signal-spirit", "signal", "0.14.0", SIGNAL_SCHEMA)
        .with_module_source("spirit", "sema", "0.7.0", SEMA_SCHEMA);
    let artifact = SchemaSourceArtifact::from_schema_text(SEMA_SCHEMA).expect("decode sema");
    let sema = SchemaEngine::default()
        .lower_schema_source_with_resolver(
            artifact.source(),
            SchemaIdentity::new("spirit:sema", "0.7.0"),
            &resolver,
        )
        .expect("lower sema");
    assert_eq!(sema.families().len(), 2);
}

#[test]
fn generated_rust_matches_the_current_schema_surface() {
    let signal = signal_spirit::SIGNAL_RUST_SOURCE;
    let nexus = include_str!("../src/schema/nexus.rs");
    let sema = include_str!("../src/schema/sema.rs");
    assert!(signal.contains("pub struct Entry {"));
    assert!(signal.contains("pub struct Intent(DomainScopes);"));
    assert!(signal.contains("pub struct TextSearch(SearchText);"));
    assert!(nexus.contains("pub struct GuardRecord(RecordRequest);"));
    assert!(sema.contains("pub struct Migration {"));
    assert!(sema.contains("pub enum RecordFamily {"));
    for retired in ["ChangeCertainty", "RegisterReferent", "ReferentsFamily"] {
        assert!(!signal.contains(retired));
        assert!(!nexus.contains(retired));
        assert!(!sema.contains(retired));
    }
}

#[test]
fn split_schemas_carry_no_retired_at_sigil() {
    for source in [SIGNAL_SCHEMA, DOMAIN_SCHEMA, NEXUS_SCHEMA, SEMA_SCHEMA] {
        assert!(!source.contains('@'));
    }
}
