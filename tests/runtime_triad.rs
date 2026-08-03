mod support;

use spirit::{
    Engine, Store,
    schema::{
        nexus::NexusWork,
        sema::{ReadInput as SemaReadInput, ReadOutput as SemaReadOutput, SemaEngine},
        signal::{
            DataLeaf, Description, Domain, DomainMatch, DomainScopes, Entry, Importance,
            ImportanceBump, ImportanceSelection, Input, Justification, KeywordMatch, Kind,
            Magnitude, Output, Query, QuoteText, Reasoning, RecordIdentifier, RecordRequest,
            SearchText, SelectedKind, Software, Technology, Testimony, TextMatch, VerbatimQuote,
        },
    },
};
use support::domain_fixtures;
use tempfile::TempDir;

struct Fixture {
    _directory: TempDir,
    path: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = TempDir::new().expect("tempdir");
        let path = directory.path().join("runtime-triad.sema");
        Self {
            _directory: directory,
            path,
        }
    }

    fn engine(&self) -> Engine {
        Engine::new(Store::open(&self.path).expect("open store"))
    }

    fn store(&self) -> Store {
        Store::open(&self.path).expect("open store")
    }
}

fn entry(description: &str, importance: Magnitude) -> Entry {
    Entry {
        domains: domain_fixtures::domains(&["runtime-triad"]),
        kind: Kind::Decision,
        description: Description::new(description),
        importance: Importance::new(importance),
    }
}

fn data_entry(domain: DataLeaf, description: &str, importance: Magnitude) -> Entry {
    Entry {
        domains: spirit::schema::signal::Domains::new(vec![Domain::Technology(
            Technology::Software(Software::Data(domain)),
        )]),
        ..entry(description, importance)
    }
}

fn justification(statement: &str) -> Justification {
    Justification {
        testimony: Testimony::new(vec![VerbatimQuote::new(QuoteText::new(statement), None)]),
        reasoning: Reasoning::new(statement),
    }
}

fn record_input(entry: Entry) -> Input {
    let statement = entry.description.payload().clone();
    Input::record(RecordRequest {
        entry,
        justification: justification(&statement),
    })
}

fn query(importance_selection: ImportanceSelection) -> Query {
    Query {
        domain_match: DomainMatch::Any,
        keyword_match: KeywordMatch::Any,
        text_match: TextMatch::Any,
        selected_kind: SelectedKind::new(Some(Kind::Decision)),
        importance_selection,
    }
}

fn accepted_identifier(output: Output) -> RecordIdentifier {
    match output {
        Output::RecordAccepted(identifier) => identifier.into_payload(),
        other => panic!("expected RecordAccepted, got {other:?}"),
    }
}

fn observed_descriptions(output: &Output) -> Vec<&str> {
    let Output::RecordsObserved(records) = output else {
        panic!("expected RecordsObserved, got {output:?}");
    };
    records
        .payload()
        .payload()
        .iter()
        .map(|record| record.entry.description.payload().as_str())
        .collect()
}

#[test]
fn generated_roots_implement_the_shared_runtime_roles() {
    fn nexus_work<Work: triad_runtime::NexusWork>() {}
    fn sema_read_input<Input: triad_runtime::SemaReadInput>() {}
    fn sema_read_output<Output: triad_runtime::SemaReadOutput>() {}
    nexus_work::<NexusWork>();
    sema_read_input::<SemaReadInput>();
    sema_read_output::<SemaReadOutput>();
}

#[test]
fn record_observe_lookup_and_count_cross_the_full_runtime() {
    let fixture = Fixture::new();
    let mut engine = fixture.engine();
    let first = accepted_identifier(
        engine
            .handle(record_input(entry("first current record", Magnitude::Low)))
            .into_root(),
    );
    accepted_identifier(
        engine
            .handle(record_input(entry(
                "second current record",
                Magnitude::High,
            )))
            .into_root(),
    );

    let observed = engine.handle(Input::observe(query(ImportanceSelection::Any)));
    let stash = match observed.root() {
        Output::RecordsStashed(stashed) => {
            assert_eq!(*stashed.record_count.payload(), 2);
            stashed.stash_handle.clone()
        }
        other => panic!("expected RecordsStashed, got {other:?}"),
    };
    let recovered = engine.handle(Input::lookup_stash(stash));
    assert_eq!(
        observed_descriptions(recovered.root()),
        vec!["second current record", "first current record"]
    );

    match engine.handle(Input::lookup(first.clone())).root() {
        Output::RecordFound(record) => {
            assert_eq!(record.record_identifier, first);
            assert_eq!(record.entry.description.payload(), "first current record");
        }
        other => panic!("expected RecordFound, got {other:?}"),
    }
    match engine
        .handle(Input::count(query(ImportanceSelection::Any)))
        .root()
    {
        Output::RecordsCounted(count) => {
            assert_eq!(*count.payload().payload().payload(), 2);
        }
        other => panic!("expected RecordsCounted, got {other:?}"),
    }
}

#[test]
fn text_search_and_intent_read_every_current_record_in_scope() {
    let fixture = Fixture::new();
    let mut engine = fixture.engine();
    for record in [
        data_entry(
            DataLeaf::All,
            "routing protocol architecture",
            Magnitude::Maximum,
        ),
        data_entry(
            DataLeaf::Persistence,
            "routing fallback note",
            Magnitude::Medium,
        ),
        data_entry(
            DataLeaf::SchemaEvolution,
            "schema evolution note",
            Magnitude::Low,
        ),
    ] {
        accepted_identifier(engine.handle(record_input(record)).into_root());
    }

    let searched = engine.handle(Input::text_search(SearchText::new("routing protocol")));
    assert_eq!(
        observed_descriptions(searched.root()),
        vec!["routing protocol architecture", "routing fallback note"]
    );

    let intent = engine.handle(Input::intent(DomainScopes::new(vec![
        Domain::Technology(Technology::Software(Software::Data(DataLeaf::Persistence))).into(),
        Domain::Technology(Technology::Software(Software::Data(
            DataLeaf::SchemaEvolution,
        )))
        .into(),
    ])));
    assert_eq!(
        observed_descriptions(intent.root()),
        vec![
            "routing protocol architecture",
            "routing fallback note",
            "schema evolution note"
        ]
    );
}

#[test]
fn importance_filtering_and_bump_are_the_only_magnitude_state_path() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let low = store
        .record_entry(entry("bump target", Magnitude::Low))
        .expect("record low entry")
        .record_identifier;
    store
        .record_entry(entry("already high", Magnitude::High))
        .expect("record high entry");

    let high_only = Query {
        importance_selection: ImportanceSelection::at_least_importance(Importance::new(
            Magnitude::High,
        )),
        ..query(ImportanceSelection::Any)
    };
    let observed = SemaEngine::observe(
        &store,
        SemaReadInput::observe(high_only)
            .with_origin_route(spirit::schema::sema::OriginRoute::new(1)),
    );
    match observed.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "already high"
            );
        }
        other => panic!("expected Observed, got {other:?}"),
    }

    drop(store);
    let mut engine = fixture.engine();
    match engine
        .handle(Input::bump_importance(ImportanceBump::new(low.clone())))
        .root()
    {
        Output::ImportanceBumped(_) => {}
        other => panic!("expected ImportanceBumped, got {other:?}"),
    }
    match engine.handle(Input::lookup(low)).root() {
        Output::RecordFound(record) => {
            assert_eq!(record.entry.importance, Importance::new(Magnitude::Medium));
        }
        other => panic!("expected RecordFound, got {other:?}"),
    }
}
