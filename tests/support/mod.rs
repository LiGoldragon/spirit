pub mod domain_fixtures {
    use spirit::schema::signal::{
        DataLeaf, Domain, DomainScope, DomainScopes, Domains, Governance, Information, Knowledge,
        ObservabilityLeaf, Software, Technology,
    };

    pub fn domains(labels: &[&str]) -> Domains {
        Domains::new(
            labels
                .iter()
                .filter(|label| !label.is_empty())
                .map(|label| domain_for_label(label))
                .collect(),
        )
    }

    #[allow(dead_code)]
    pub fn scopes(labels: &[&str]) -> DomainScopes {
        DomainScopes::new(
            labels
                .iter()
                .map(|label| DomainScope::from(domain_for_label(label)))
                .collect(),
        )
    }

    fn domain_for_label(label: &str) -> Domain {
        let normalized = label.to_ascii_lowercase();
        if normalized.contains("schema") {
            return Domain::Technology(Technology::Software(Software::Data(
                DataLeaf::SchemaEvolution,
            )));
        }
        if normalized.contains("runtime") {
            return Domain::Technology(Technology::Software(Software::Data(DataLeaf::Modeling)));
        }
        if normalized.contains("migration") {
            return Domain::Technology(Technology::Software(Software::Data(DataLeaf::Migration)));
        }
        if normalized.contains("trace") || normalized.contains("stream") {
            return Domain::Technology(Technology::Software(Software::Observability(
                ObservabilityLeaf::Tracing,
            )));
        }
        if normalized.contains("govern") {
            return Domain::Governance(Governance::Government);
        }
        if normalized.contains("meaning") || normalized.contains("relating") {
            return Domain::Knowledge(Knowledge::Philosophy);
        }
        Domain::Information(Information::Documentation)
    }
}
