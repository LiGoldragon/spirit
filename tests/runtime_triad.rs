#[cfg(feature = "nota-text")]
use spirit::schema::signal::{Export, Import};
use spirit::{
    Engine, Nexus, SignalAdmission, Store,
    schema::{
        nexus::{self, CommandSemaWrite, NexusAction, NexusEffectCommand, NexusEngine, NexusWork},
        sema::{
            self, ReadInput as SemaReadInput, ReadOutput as SemaReadOutput, SemaEngine,
            WriteInput as SemaWriteInput, WriteOutput as SemaWriteOutput,
        },
        signal::{
            CertaintyChange, DatabaseMarker, Entry, ErrorReport, Input, Kind, Magnitude,
            MailLedgerEvent, MessageIdentifier, MessageSent, MessageSentHook, OriginRoute, Output,
            PrivacySelection, ProcessedMail, Query, RecordChange, RecordIdentifier, SemaReceipt,
            SentMail, SignalEngine, SignalRejection, StashHandle, Statement, TopicMatch,
            ValidationError,
        },
    },
};
use tempfile::TempDir;

struct SemaFile {
    #[allow(dead_code)]
    directory: TempDir,
    path: std::path::PathBuf,
}

impl SemaFile {
    fn new() -> Self {
        let directory = TempDir::new().expect("tempdir");
        let path = directory.path().join("runtime-triad.sema");
        Self { directory, path }
    }

    fn open_store(&self) -> Store {
        Store::open(&self.path).expect("open sema store")
    }

    fn engine(&self) -> Engine {
        Engine::new(self.open_store())
    }
}

struct SentHookProbe {
    events: Vec<MailLedgerEvent>,
}

impl MessageSentHook for SentHookProbe {
    type Error = std::convert::Infallible;

    fn message_sent(&mut self, event: MessageSent) -> Result<(), Self::Error> {
        self.events.push(event.into_mail_ledger_event());
        Ok(())
    }
}

fn entry(description: &str) -> Entry {
    entry_with_topics(&["runtime-triad"], description)
}

fn entry_with_topics(topics: &[&str], description: &str) -> Entry {
    Entry {
        topics: topics.iter().map(|topic| String::from(*topic)).collect(),
        kind: Kind::Decision,
        description: String::from(description),
        magnitude: Magnitude::Maximum,
        privacy: Magnitude::Zero,
    }
}

fn entry_with_privacy(description: &str, privacy: Magnitude) -> Entry {
    Entry {
        privacy,
        ..entry(description)
    }
}

fn query() -> Query {
    full_query(&["runtime-triad"], Some(Kind::Decision))
}

fn full_query(topics: &[&str], kind: Option<Kind>) -> Query {
    Query {
        topic_match: TopicMatch::full(topics.iter().map(|topic| String::from(*topic)).collect()),
        kind,
        privacy_selection: PrivacySelection::default_observation_privacy(),
    }
}

fn partial_query(topics: &[&str], kind: Option<Kind>) -> Query {
    Query {
        topic_match: TopicMatch::partial(topics.iter().map(|topic| String::from(*topic)).collect()),
        kind,
        privacy_selection: PrivacySelection::default_observation_privacy(),
    }
}

fn privacy_query(privacy_selection: PrivacySelection) -> Query {
    Query {
        privacy_selection,
        ..query()
    }
}

fn route(offset: u64) -> OriginRoute {
    OriginRoute(1_000_000 + offset)
}

fn nexus_route(offset: u64) -> spirit::schema::nexus::OriginRoute {
    route(offset).into()
}

fn execute_nexus(nexus: &mut Nexus, input: nexus::Nexus<NexusWork>) -> nexus::Nexus<NexusAction> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("nexus test runtime")
        .block_on(NexusEngine::execute(nexus, input))
}

fn sema_route(offset: u64) -> spirit::schema::sema::OriginRoute {
    route(offset).into()
}

fn sema_write_message(input: SemaWriteInput, offset: u64) -> sema::Sema<sema::WriteInput> {
    input.with_origin_route(sema_route(offset))
}

fn sema_read_message(input: SemaReadInput, offset: u64) -> sema::Sema<sema::ReadInput> {
    input.with_origin_route(sema_route(offset))
}

fn input_record(entry: Entry) -> Input {
    Input::record(entry)
}

fn input_state(statement: &str) -> Input {
    Input::state(Statement::new(String::from(statement)))
}

fn input_observe(query: Query) -> Input {
    Input::observe(query)
}

fn input_lookup(record_identifier: RecordIdentifier) -> Input {
    Input::lookup(record_identifier)
}

fn input_count(query: Query) -> Input {
    Input::count(query)
}

fn input_change_certainty(record_identifier: RecordIdentifier, certainty: Magnitude) -> Input {
    Input::change_certainty(CertaintyChange {
        record_identifier,
        certainty,
    })
}

fn input_change_record(record_identifier: RecordIdentifier, entry: Entry) -> Input {
    Input::change_record(RecordChange {
        record_identifier,
        entry,
    })
}

fn input_lookup_stash(stash_handle: StashHandle) -> Input {
    Input::lookup_stash(stash_handle)
}

fn nexus_signal_arrived(input: Input) -> NexusWork {
    NexusWork::signal_arrived(input)
}

fn sema_record(entry: Entry) -> SemaWriteInput {
    SemaWriteInput::record(entry)
}

fn sema_remove(record_identifier: RecordIdentifier) -> SemaWriteInput {
    SemaWriteInput::remove(record_identifier)
}

fn sema_change_certainty(
    record_identifier: RecordIdentifier,
    certainty: Magnitude,
) -> SemaWriteInput {
    SemaWriteInput::change_certainty(CertaintyChange {
        record_identifier,
        certainty,
    })
}

fn sema_change_record(record_identifier: RecordIdentifier, entry: Entry) -> SemaWriteInput {
    SemaWriteInput::change_record(RecordChange {
        record_identifier,
        entry,
    })
}

fn sema_observe(query: Query) -> SemaReadInput {
    SemaReadInput::observe(query)
}

fn sema_lookup(record_identifier: RecordIdentifier) -> SemaReadInput {
    SemaReadInput::lookup(record_identifier)
}

fn sema_count(query: Query) -> SemaReadInput {
    SemaReadInput::count(query)
}

#[test]
fn generated_plane_roots_implement_shared_triad_runtime_roles() {
    fn assert_nexus_work<Work: triad_runtime::NexusWork>() {}
    fn assert_nexus_effect_command<Effect: triad_runtime::NexusEffectCommand>() {}
    fn assert_nexus_effect_result<Result: triad_runtime::NexusEffectResult>() {}
    fn assert_sema_write_input<Input: triad_runtime::SemaWriteInput>() {}
    fn assert_sema_write_output<Output: triad_runtime::SemaWriteOutput>() {}
    fn assert_sema_read_input<Input: triad_runtime::SemaReadInput>() {}
    fn assert_sema_read_output<Output: triad_runtime::SemaReadOutput>() {}
    fn assert_nexus_action<
        Action: triad_runtime::NexusAction<
                Reply = Output,
                SemaWrite = CommandSemaWrite,
                SemaRead = SemaReadInput,
                Effect = NexusEffectCommand,
                Work = NexusWork,
            >,
    >() {
    }

    assert_nexus_work::<NexusWork>();
    assert_nexus_effect_command::<NexusEffectCommand>();
    assert_nexus_effect_result::<spirit::schema::nexus::NexusEffectResult>();
    assert_sema_write_input::<CommandSemaWrite>();
    assert_sema_write_input::<SemaWriteInput>();
    assert_sema_write_output::<SemaWriteOutput>();
    assert_sema_read_input::<SemaReadInput>();
    assert_sema_read_output::<SemaReadOutput>();
    assert_nexus_action::<NexusAction>();
}

#[test]
fn nexus_runner_loop_routes_record_input_to_sema_write_command_then_back_to_reply() {
    // Designer 480 / operator 287 §"Recursive Computation": the runner
    // loop replaces the old projection chain. NexusEngine::execute drives
    // SignalArrived(Record) → CommandSemaWrite → SemaWriteCompleted →
    // ReplyToSignal(RecordAccepted). The trace property is observable on
    // the Nexus instance after execution.
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_record(entry("nexus runner routes")))
        .with_origin_route(nexus_route(1));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);

    assert_eq!(nexus_output.origin_route(), nexus_route(1));
    match nexus_output.root() {
        NexusAction::ReplyToSignal(Output::RecordAccepted(receipt)) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
        }
        other => panic!("expected ReplyToSignal(RecordAccepted), got {other:?}"),
    }
    assert_eq!(nexus.store().len(), 1, "runner loop committed to SEMA");
}

#[test]
fn nexus_classifies_state_into_provisional_record_through_sema_write() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_state("capture this statement"))
        .with_origin_route(nexus_route(2));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);

    match nexus_output.root() {
        NexusAction::ReplyToSignal(Output::RecordAccepted(receipt)) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
        }
        other => panic!("expected State to classify into RecordAccepted, got {other:?}"),
    }

    let observed = SemaEngine::observe(
        nexus.store(),
        sema_read_message(
            sema_observe(full_query(&["unclassified"], Some(Kind::Clarification))),
            3,
        ),
    );
    match observed.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.record_set.len(), 1);
            assert_eq!(records.record_set[0].description, "capture this statement");
            assert_eq!(records.record_set[0].magnitude, Magnitude::Minimum);
            assert_eq!(records.record_set[0].privacy, Magnitude::Zero);
        }
        other => panic!("expected classified State record to be observable, got {other:?}"),
    }
}

#[test]
fn nexus_change_certainty_is_visible_as_schema_declared_write_command() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_change_certainty(1, Magnitude::Zero))
        .with_origin_route(nexus_route(5));

    let first_action = NexusEngine::decide(&mut nexus, nexus_input);

    assert_eq!(first_action.origin_route(), nexus_route(5));
    match first_action.root() {
        NexusAction::CommandSemaWrite(CommandSemaWrite::ChangeCertainty(change)) => {
            assert_eq!(change.record_identifier, 1);
            assert_eq!(change.certainty, Magnitude::Zero);
        }
        other => panic!(
            "expected ChangeCertainty to become CommandSemaWrite(ChangeCertainty), got {other:?}"
        ),
    }
}

#[test]
fn nexus_change_record_is_visible_as_schema_declared_write_command() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let replacement = entry_with_topics(&["runtime-triad", "replacement"], "replacement record");
    let nexus_input = nexus_signal_arrived(input_change_record(1, replacement.clone()))
        .with_origin_route(nexus_route(6));

    let first_action = NexusEngine::decide(&mut nexus, nexus_input);

    assert_eq!(first_action.origin_route(), nexus_route(6));
    match first_action.root() {
        NexusAction::CommandSemaWrite(CommandSemaWrite::ChangeRecord(change)) => {
            assert_eq!(change.record_identifier, 1);
            assert_eq!(change.entry, replacement);
        }
        other => {
            panic!("expected ChangeRecord to become CommandSemaWrite(ChangeRecord), got {other:?}")
        }
    }
}

#[test]
fn nexus_state_classification_is_visible_as_schema_declared_effect_command() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_state("visible classification"))
        .with_origin_route(nexus_route(4));

    let first_action = NexusEngine::decide(&mut nexus, nexus_input);

    assert_eq!(first_action.origin_route(), nexus_route(4));
    match first_action.root() {
        NexusAction::CommandEffect(NexusEffectCommand::ClassifyState(statement)) => {
            assert_eq!(statement.payload(), "visible classification");
        }
        other => panic!("expected State to become CommandEffect(ClassifyState), got {other:?}"),
    }
    assert_eq!(
        nexus.store().len(),
        0,
        "the first Nexus decision exposes classification before durable SEMA write"
    );
}

#[test]
fn sema_engine_changes_certainty_without_changing_record_identifier() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(sema_record(entry("certainty target")), 1),
    );

    let changed = SemaEngine::apply(
        &mut store,
        sema_write_message(sema_change_certainty(1, Magnitude::Zero), 2),
    );
    match changed.root() {
        SemaWriteOutput::CertaintyChanged(receipt) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.certainty, Magnitude::Zero);
            assert_eq!(receipt.database_marker.commit_sequence, 2);
        }
        other => panic!("expected CertaintyChanged receipt, got {other:?}"),
    }

    let found = SemaEngine::observe(&store, sema_read_message(sema_lookup(1), 3));
    match found.root() {
        SemaReadOutput::Found(record) => {
            assert_eq!(record.record_identifier, 1);
            assert_eq!(record.entry.description, "certainty target");
            assert_eq!(record.entry.magnitude, Magnitude::Zero);
        }
        other => panic!("expected changed record lookup, got {other:?}"),
    }
}

#[test]
fn sema_engine_changes_record_without_changing_record_identifier() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(sema_record(entry("original target")), 1),
    );

    let replacement = entry_with_topics(&["runtime-triad", "replacement"], "replacement target");
    let changed = SemaEngine::apply(
        &mut store,
        sema_write_message(sema_change_record(1, replacement.clone()), 2),
    );
    match changed.root() {
        SemaWriteOutput::RecordChanged(receipt) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.database_marker.commit_sequence, 2);
        }
        other => panic!("expected RecordChanged receipt, got {other:?}"),
    }

    let found = SemaEngine::observe(&store, sema_read_message(sema_lookup(1), 3));
    match found.root() {
        SemaReadOutput::Found(record) => {
            assert_eq!(record.record_identifier, 1);
            assert_eq!(record.entry, replacement);
        }
        other => panic!("expected changed record lookup, got {other:?}"),
    }
}

#[test]
fn signal_admission_pushes_accepted_message_through_sent_hook_before_nexus_holds_mail() {
    let signal_admission = SignalAdmission::default();
    let signal_entry = entry("signal pushes to nexus");
    let expected_short_header = input_record(signal_entry.clone()).short_header();
    let accepted = signal_admission
        .admit(input_record(signal_entry.clone()))
        .expect("signal input admits");
    let mut hook = SentHookProbe { events: Vec::new() };
    let expected_sent = MailLedgerEvent::Sent(SentMail {
        mail_identifier: 1,
        origin_route: route(1),
        short_header: expected_short_header,
    });

    assert_eq!(accepted.message_sent().identifier, MessageIdentifier(1));
    assert_eq!(accepted.message_sent().origin_route(), route(1));
    assert_ne!(
        accepted.message_sent().origin_route(),
        OriginRoute(accepted.message_sent().identifier.0)
    );
    assert_eq!(hook.events, []);

    // The sent hook fires at the Signal -> Nexus handoff, witnessed by a
    // schema-emitted mail ledger event.
    accepted
        .message_sent()
        .push_to(&mut hook)
        .expect("sent hook fires");

    assert_eq!(
        hook.events,
        vec![expected_sent],
        "hook witness must be a schema-emitted mail ledger event"
    );
}

#[test]
fn nexus_step_decide_routes_signal_arrival_to_sema_command_without_committing() {
    // Designer 480: the runner loop's step plane is now visible through
    // Nexus's hand-written decision center. A SignalArrived(Record) becomes
    // a CommandSemaWrite action; SEMA only commits when the runner spends
    // the action against SemaEngine::apply.
    let sema = SemaFile::new();
    let store = sema.open_store();
    assert!(store.is_empty(), "store starts empty");

    // Trace the runner's first action without driving it.
    let signal_admission = SignalAdmission::default();
    let signal_input = input_record(entry("held in flight")).with_origin_route(route(1));
    let nexus_input = SignalEngine::triage(&signal_admission, signal_input);
    assert_eq!(nexus_input.origin_route(), nexus_route(1));
    match nexus_input.root() {
        NexusWork::SignalArrived(Input::Record(recorded)) => {
            assert_eq!(recorded.description, "held in flight")
        }
        other => panic!("expected SignalArrived(Record), got {other:?}"),
    }

    assert!(
        store.is_empty(),
        "triage runs without committing to the durable SEMA store"
    );
}

#[test]
fn sema_engine_writes_durable_records_and_returns_schema_objects() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    let operation = sema_write_message(sema_record(entry("SEMA writes durable facts")), 1);

    let response = SemaEngine::apply(&mut store, operation);

    assert_eq!(response.origin_route(), sema_route(1));
    match response.root() {
        SemaWriteOutput::Recorded(receipt) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
            assert_ne!(
                receipt.database_marker.state_digest, 0,
                "a committed record yields a non-empty content digest"
            );
        }
        other => panic!("expected schema-emitted Recorded receipt, got {other:?}"),
    }
    assert_eq!(store.len(), 1);
    // PROOF the .sema file is real on disk.
    assert!(store.path().exists(), "the .sema database file exists");
}

#[test]
fn engine_lifecycle_runs_generated_trait_hooks_without_actor_mailboxes() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    engine.start().expect("generated lifecycle start hooks run");
    let output = engine.handle(input_record(entry("lifecycle still handles input")));
    engine.stop().expect("generated lifecycle stop hooks run");

    match output.root() {
        Output::RecordAccepted(receipt) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
        }
        other => panic!("expected lifecycle-wrapped engine to accept record, got {other:?}"),
    }
}

#[test]
fn nexus_engine_trait_runs_nexus_decision_through_sema_state() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_record(entry("nexus trait root")))
        .with_origin_route(nexus_route(20));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);

    assert_eq!(nexus_output.origin_route(), nexus_route(20));
    match nexus_output.root() {
        NexusAction::ReplyToSignal(Output::RecordAccepted(receipt)) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
        }
        other => panic!("expected NexusEngine to return a Signal reply root, got {other:?}"),
    }
    assert_eq!(
        nexus.store().len(),
        1,
        "NexusEngine is the computation plane that invokes SEMA for database work"
    );
}

#[test]
fn signal_engine_trait_triages_signal_roots_to_nexus_and_back() {
    let sema = SemaFile::new();
    let signal_admission = SignalAdmission::default();
    let mut nexus = Nexus::new(sema.open_store());
    let signal_input = input_record(entry("signal trait root")).with_origin_route(route(21));

    let nexus_input = SignalEngine::triage(&signal_admission, signal_input);
    assert_eq!(nexus_input.origin_route(), nexus_route(21));
    assert!(matches!(
        nexus_input.root(),
        NexusWork::SignalArrived(Input::Record(_))
    ));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);
    let signal_output = SignalEngine::reply(&signal_admission, nexus_output);

    assert_eq!(signal_output.origin_route(), route(21));
    match signal_output.root() {
        Output::RecordAccepted(receipt) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
        }
        other => panic!("expected SignalEngine to return a Signal output root, got {other:?}"),
    }
    assert_eq!(nexus.store().len(), 1);
}

#[test]
fn sema_store_persists_records_across_reopen_of_the_same_sema_file() {
    // The durability proof (record 1007/1008, bead primary-q2au): write
    // records, drop the store, reopen from the same `.sema` path, and
    // observe the records survive with their commit ledger intact.
    let sema = SemaFile::new();

    let first_marker;
    {
        let mut store = sema.open_store();
        SemaEngine::apply(
            &mut store,
            sema_write_message(sema_record(entry("durable one")), 1),
        );
        let recorded = SemaEngine::apply(
            &mut store,
            sema_write_message(sema_record(entry("durable two")), 2),
        );
        first_marker = match recorded.into_root() {
            SemaWriteOutput::Recorded(receipt) => receipt.database_marker,
            other => panic!("expected Recorded, got {other:?}"),
        };
        assert_eq!(store.len(), 2);
        // store drops here, releasing the sema-engine file handle.
    }

    // Reopen from the SAME path — a fresh process would do exactly this.
    let mut reopened = sema.open_store();
    assert_eq!(
        reopened.len(),
        2,
        "records written before the drop survive the reopen"
    );

    // The commit ledger resumed: the next write is commit sequence 3,
    // not 1, proving the counter persisted, not just the records.
    let after = SemaEngine::apply(
        &mut reopened,
        sema_write_message(sema_record(entry("durable three")), 3),
    );
    match after.root() {
        SemaWriteOutput::Recorded(receipt) => {
            assert_eq!(receipt.record_identifier, 3);
            assert_eq!(receipt.database_marker.commit_sequence, 3);
        }
        other => panic!("expected Recorded after reopen, got {other:?}"),
    }

    // An Observe against the reopened store finds a record written before
    // the drop, through the schema-emitted query path.
    let observed = SemaEngine::observe(&reopened, sema_read_message(sema_observe(query()), 4));
    assert!(
        matches!(observed.root(), SemaReadOutput::Observed(_)),
        "the reopened store observes a pre-drop record"
    );
    assert_ne!(
        first_marker.state_digest, 0,
        "the durable digest is content-addressed, not a zero placeholder"
    );
}

#[test]
fn sema_engine_queries_partial_and_full_topic_sets() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_topics(&["runtime-triad", "schema"], "both")),
            1,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_topics(&["runtime-triad"], "runtime only")),
            2,
        ),
    );

    let partial = SemaEngine::observe(
        &store,
        sema_read_message(sema_observe(partial_query(&["schema", "other"], None)), 3),
    );
    match partial.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.record_set.len(), 1);
            assert_eq!(records.record_set[0].description, "both");
        }
        other => panic!("expected partial query to observe one record, got {other:?}"),
    }

    let full = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(full_query(
                &["runtime-triad", "schema"],
                Some(Kind::Decision),
            )),
            4,
        ),
    );
    match full.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.record_set.len(), 1);
            assert_eq!(records.record_set[0].description, "both");
        }
        other => panic!("expected full query to require every topic, got {other:?}"),
    }
}

#[test]
fn sema_engine_queries_privacy_as_directional_magnitude() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(sema_record(entry("open")), 1),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_privacy("private", Magnitude::High)),
            2,
        ),
    );

    let default = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(privacy_query(
                PrivacySelection::default_observation_privacy(),
            )),
            3,
        ),
    );
    let any = SemaEngine::observe(
        &store,
        sema_read_message(sema_observe(privacy_query(PrivacySelection::Any)), 4),
    );
    let high = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(privacy_query(PrivacySelection::at_least(Magnitude::High))),
            5,
        ),
    );

    match default.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.record_set.len(), 1);
            assert_eq!(records.record_set[0].description, "open");
        }
        other => panic!("expected default privacy query to observe open record, got {other:?}"),
    }
    match any.root() {
        SemaReadOutput::Observed(records) => assert_eq!(records.record_set.len(), 2),
        other => panic!("expected any privacy query to observe both records, got {other:?}"),
    }
    match high.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.record_set.len(), 1);
            assert_eq!(records.record_set[0].description, "private");
        }
        other => panic!("expected high privacy query to observe private record, got {other:?}"),
    }
}

#[test]
fn sema_engine_lookup_and_count_are_read_plane_operations() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_topics(&["runtime-triad", "schema"], "first")),
            1,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_topics(&["runtime-triad"], "second")),
            2,
        ),
    );

    let found = SemaEngine::observe(&store, sema_read_message(sema_lookup(2), 3));
    match found.root() {
        SemaReadOutput::Found(record) => {
            assert_eq!(record.record_identifier, 2);
            assert_eq!(record.entry.description, "second");
            assert_eq!(record.database_marker.commit_sequence, 2);
        }
        other => panic!("expected SEMA lookup to find the second record, got {other:?}"),
    }

    let counted = SemaEngine::observe(
        &store,
        sema_read_message(sema_count(partial_query(&["runtime-triad"], None)), 4),
    );
    match counted.root() {
        SemaReadOutput::Counted(records) => {
            assert_eq!(records.record_count, 2);
            assert_eq!(records.database_marker.commit_sequence, 2);
        }
        other => panic!("expected SEMA count to return two records, got {other:?}"),
    }
}

#[test]
fn sema_engine_observes_through_shared_reference_for_parallel_readers() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_topics(&["runtime-triad", "schema"], "parallel")),
            1,
        ),
    );

    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|offset| {
                let store = &store;
                scope.spawn(move || {
                    SemaEngine::observe(
                        store,
                        sema_read_message(
                            sema_observe(full_query(&["runtime-triad"], None)),
                            10 + offset,
                        ),
                    )
                })
            })
            .collect();

        for worker in workers {
            let observed = worker.join().expect("reader thread joins");
            assert!(matches!(observed.root(), SemaReadOutput::Observed(_)));
        }
    });
}

#[test]
fn sema_engine_removes_records_and_advances_database_work_marker() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(sema_record(entry("remove target")), 1),
    );

    let removed = SemaEngine::apply(&mut store, sema_write_message(sema_remove(1), 2));
    let removed_marker = match removed.root() {
        SemaWriteOutput::Removed(receipt) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.database_marker.commit_sequence, 2);
            receipt.database_marker.clone()
        }
        other => panic!("expected Removed receipt, got {other:?}"),
    };

    let observed = SemaEngine::observe(&store, sema_read_message(sema_observe(query()), 3));
    assert!(
        matches!(observed.root(), SemaReadOutput::Missed(_)),
        "removed record should not be observed again"
    );
    assert_eq!(store.len(), 0);
    assert_eq!(removed_marker.state_digest, 0);
}

#[test]
fn nexus_runs_sema_while_holding_mail_then_replies_through_schema_objects() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let recorded = engine.handle(input_record(entry("nexus drives sema")));
    assert_eq!(recorded.origin_route(), route(1));
    match recorded.root() {
        Output::RecordAccepted(receipt) => {
            assert_eq!(receipt.record_identifier, 1);
            assert_eq!(receipt.database_marker.commit_sequence, 1);
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    }
    assert_eq!(engine.sent_message_count(), 1);
    assert_eq!(engine.processed_message_count(), 1);
    assert_eq!(engine.record_count(), 1);
}

#[test]
fn signal_admission_rejects_invalid_input_with_schema_emitted_rejection_before_mail_or_sema() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();
    let mut bad = entry("missing topic");
    bad.topics = vec![String::new()];

    let output = engine.handle(input_record(bad));

    assert_eq!(
        output.root(),
        &Output::rejected(SignalRejection {
            validation_error: ValidationError::EmptyTopic,
            database_marker: DatabaseMarker {
                commit_sequence: 0,
                state_digest: 0,
            },
        })
    );
    assert_eq!(output.origin_route(), route(1));
    assert_eq!(engine.record_count(), 0);
    assert_eq!(engine.sent_message_count(), 0);
    assert_eq!(engine.processed_message_count(), 0);
    assert_eq!(engine.mail_ledger(), []);
}

#[test]
fn sema_read_miss_completion_routes_through_runner_loop_to_error_reply() {
    // Designer 480: the SemaReadCompleted(Missed) fact becomes
    // ReplyToSignal(Error) through the runner loop's decide step.
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = NexusWork::sema_read_completed(SemaReadOutput::missed(ErrorReport {
        error_message: String::from("no matching record"),
        database_marker: DatabaseMarker {
            commit_sequence: 0,
            state_digest: 0,
        },
    }))
    .with_origin_route(nexus_route(7));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);
    let signal_output = SignalEngine::reply(&SignalAdmission::default(), nexus_output);

    assert_eq!(
        signal_output.root(),
        &Output::error(ErrorReport {
            error_message: String::from("no matching record"),
            database_marker: DatabaseMarker {
                commit_sequence: 0,
                state_digest: 0,
            },
        })
    );
    assert_eq!(signal_output.origin_route(), route(7));
}

#[test]
fn plane_envelopes_keep_payload_names_scoped() {
    // Designer 480: plane envelopes still enforce the per-plane scoping
    // of payload names. The Nexus envelope around a CommandSemaWrite
    // action is a distinct generated type from the Signal output
    // envelope that carries the eventual reply.
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_record(entry("language input")))
        .with_origin_route(nexus_route(11));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);
    assert_eq!(nexus_output.origin_route(), nexus_route(11));
    // The runner loop drove the full cycle to ReplyToSignal(RecordAccepted).
    assert!(matches!(
        nexus_output.root(),
        NexusAction::ReplyToSignal(Output::RecordAccepted(_))
    ));

    let signal_output = SignalEngine::reply(&SignalAdmission::default(), nexus_output);
    assert_eq!(signal_output.origin_route(), route(11));
    assert!(matches!(signal_output.root(), Output::RecordAccepted(_)));
}

#[test]
fn nexus_and_sema_have_explicit_input_output_languages() {
    // Designer 480: the Nexus and SEMA languages remain explicit, but the
    // step-of-decision is now driven by the runner loop. Probing one cycle
    // of NexusEngine::execute proves both languages cross cleanly.
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_record(entry("language input")))
        .with_origin_route(nexus_route(13));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);
    assert!(matches!(
        nexus_output.root(),
        NexusAction::ReplyToSignal(Output::RecordAccepted(_))
    ));

    // Standalone SEMA completion fact also routes back to the right reply.
    let sema_output = SemaWriteOutput::recorded(SemaReceipt {
        record_identifier: 3,
        database_marker: DatabaseMarker {
            commit_sequence: 4,
            state_digest: 127,
        },
    });
    let nexus_input_from_sema =
        NexusWork::sema_write_completed(sema_output).with_origin_route(nexus_route(14));
    let mut second_nexus = Nexus::new(SemaFile::new().open_store());
    let nexus_output_from_sema = execute_nexus(&mut second_nexus, nexus_input_from_sema);
    let signal_output = SignalEngine::reply(&SignalAdmission::default(), nexus_output_from_sema);
    assert!(matches!(signal_output.root(), Output::RecordAccepted(_)));
}

#[cfg(feature = "nota-text")]
#[test]
fn import_export_paths_use_single_colon_namespaces() {
    let import = Import {
        source_path: String::from("signal:sema:Magnitude"),
        local_path: String::from("spirit:core:Magnitude"),
    };
    let export = Export {
        local_path: String::from("spirit:core:SemaWriteOutput"),
        public_path: String::from("spirit:sema:SemaWriteOutput"),
    };

    assert_eq!(
        import.to_nota(),
        "([signal:sema:Magnitude] [spirit:core:Magnitude])"
    );
    assert_eq!(
        export.to_nota(),
        "([spirit:core:SemaWriteOutput] [spirit:sema:SemaWriteOutput])"
    );
}

#[test]
fn full_runtime_triad_records_then_observes_through_durable_sema_with_stash() {
    // Designer 480 layer-2 witness for the Stash effect (operator 287 §
    // "Acceptance Tests"): Observe with a non-empty result drives the
    // recursive Nexus loop through Stash and returns the slim
    // RecordsStashed reply. A follow-up LookupStash by handle returns the
    // full records. The two-call pattern proves the recursive computation
    // is real on a real flow.
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let recorded = engine.handle(input_record(entry("full runtime triad works")));
    assert_eq!(recorded.origin_route(), route(1));
    let record_marker = match recorded.root() {
        Output::RecordAccepted(receipt) => {
            assert_eq!(receipt.record_identifier, 1);
            receipt.database_marker.clone()
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    };
    assert_eq!(record_marker.commit_sequence, 1);
    assert_eq!(engine.sent_message_count(), 1);
    assert_eq!(engine.processed_message_count(), 1);

    let observed = engine.handle(input_observe(Query {
        topic_match: TopicMatch::full(vec![String::from("runtime-triad")]),
        kind: Some(Kind::Decision),
        privacy_selection: PrivacySelection::default_observation_privacy(),
    }));

    assert_eq!(observed.origin_route(), route(2));
    let stash_handle = match observed.root() {
        Output::RecordsStashed(stashed) => {
            // The slim wire reply per Spirit 1389: handle + count + marker,
            // not the full record set.
            assert_eq!(stashed.record_count, 1);
            assert_eq!(stashed.database_marker, record_marker);
            stashed.stash_handle
        }
        other => panic!("expected slim RecordsStashed reply after Observe, got {other:?}"),
    };
    assert_eq!(engine.sent_message_count(), 2);
    assert_eq!(engine.processed_message_count(), 2);

    // Layer 2 witness: follow up with LookupStash and get the full records.
    let looked_up = engine.handle(input_lookup_stash(stash_handle));
    assert_eq!(looked_up.origin_route(), route(3));
    match looked_up.root() {
        Output::RecordsObserved(records) => {
            assert_eq!(records.record_set, vec![entry("full runtime triad works")]);
        }
        other => panic!(
            "expected LookupStash to return RecordsObserved with full records, got {other:?}"
        ),
    }
    assert_eq!(engine.sent_message_count(), 3);
    assert_eq!(engine.processed_message_count(), 3);

    assert_eq!(
        engine.mail_ledger().len(),
        6,
        "three round-trips through the mail ledger: Record, Observe (with stash), LookupStash"
    );

    // First two mail-ledger events are the Record cycle.
    assert!(matches!(
        engine.mail_ledger()[0],
        MailLedgerEvent::Sent(SentMail {
            mail_identifier: 1,
            ..
        })
    ));
    assert!(matches!(
        engine.mail_ledger()[1],
        MailLedgerEvent::Processed(ProcessedMail {
            mail_identifier: 1,
            ..
        })
    ));
}

#[test]
fn full_runtime_triad_looks_up_and_counts_through_signal_nexus_and_sema() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    engine.handle(input_record(entry_with_topics(
        &["runtime-triad", "schema"],
        "lookup one",
    )));
    engine.handle(input_record(entry_with_topics(
        &["runtime-triad"],
        "lookup two",
    )));

    let found = engine.handle(input_lookup(1));
    match found.root() {
        Output::RecordFound(record) => {
            assert_eq!(record.record_identifier, 1);
            assert_eq!(record.entry.description, "lookup one");
            assert_eq!(record.database_marker.commit_sequence, 2);
        }
        other => panic!("expected full runtime lookup to return RecordFound, got {other:?}"),
    }

    let counted = engine.handle(input_count(partial_query(&["runtime-triad"], None)));
    match counted.root() {
        Output::RecordsCounted(records) => {
            assert_eq!(records.record_count, 2);
            assert_eq!(records.database_marker.commit_sequence, 2);
        }
        other => panic!("expected full runtime count to return RecordsCounted, got {other:?}"),
    }
}
