use spirit_next::{
    CommitSequence, DatabaseMarker, Description, Engine, Entry, ErrorMessage, ErrorReport, Input,
    Kind, Magnitude, MailIdentifier, MailLedgerEvent, MessageIdentifier, MessageSent,
    MessageSentHook, Nexus, NexusEngine, NexusInput, NexusOutput, OriginRoute, Output,
    ProcessedMail, Query, RecordCount, RecordIdentifier, RecordSet, SemaEngine, SemaReadInput,
    SemaReadOutput, SemaReceipt, SemaWriteInput, SemaWriteOutput, SentMail, ShortHeader,
    SignalActor, SignalEngine, SignalRejection, StateDigest, Store, Topic, TopicMatch, Topics,
    ValidationError, schema_meta, sema,
};
#[cfg(feature = "nota-text")]
use spirit_next::{Export, Import, LocalPath, PublicPath, SourcePath};
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
        topics: Topics(
            topics
                .iter()
                .map(|topic| Topic(String::from(*topic)))
                .collect(),
        ),
        kind: Kind::Decision,
        description: Description(String::from(description)),
        magnitude: Magnitude::Maximum,
    }
}

fn query() -> Query {
    full_query(&["runtime-triad"], Some(Kind::Decision))
}

fn full_query(topics: &[&str], kind: Option<Kind>) -> Query {
    Query {
        topic_match: TopicMatch::Full(Topics(
            topics
                .iter()
                .map(|topic| Topic(String::from(*topic)))
                .collect(),
        )),
        kind,
    }
}

fn partial_query(topics: &[&str], kind: Option<Kind>) -> Query {
    Query {
        topic_match: TopicMatch::Partial(Topics(
            topics
                .iter()
                .map(|topic| Topic(String::from(*topic)))
                .collect(),
        )),
        kind,
    }
}

fn route(offset: u64) -> OriginRoute {
    OriginRoute(1_000_000 + offset)
}

fn sema_write_message(input: SemaWriteInput, offset: u64) -> sema::Sema<sema::WriteInput> {
    input.with_origin_route(route(offset))
}

fn sema_read_message(input: SemaReadInput, offset: u64) -> sema::Sema<sema::ReadInput> {
    input.with_origin_route(route(offset))
}

#[test]
fn nexus_input_lowers_signal_payload_to_generated_sema_command() {
    let command = NexusInput::from(Input::Record(entry("nexus mail lowers to SEMA")))
        .with_origin_route(route(1))
        .into_nexus_output()
        .into_sema_write_input();

    let plane = schema_meta::Plane::<Input, NexusInput, SemaWriteInput>::Sema(command.clone());
    assert_eq!(plane.origin_route(), route(1));
    let schema_meta::Plane::Sema(command) = plane else {
        panic!("expected SEMA plane");
    };
    assert_eq!(command.origin_route(), route(1));
    match command.root() {
        SemaWriteInput::Record(recorded) => {
            assert_eq!(recorded.description.0, "nexus mail lowers to SEMA");
        }
        SemaWriteInput::Remove(_) => panic!("record input should lower to record command"),
    }
}

#[test]
fn signal_actor_pushes_accepted_message_through_sent_hook_before_nexus_holds_mail() {
    let signal_actor = SignalActor::default();
    let signal_entry = entry("signal pushes to nexus");
    let accepted = signal_actor
        .admit(Input::Record(signal_entry.clone()))
        .expect("signal input admits");
    let mut hook = SentHookProbe { events: Vec::new() };
    let expected_sent = MailLedgerEvent::Sent(SentMail {
        mail_identifier: MailIdentifier(1),
        origin_route: route(1),
        short_header: ShortHeader(0),
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
fn signal_input_lowers_through_schema_projections_without_touching_sema_store() {
    // The schema-emitted projection chain Signal<Input> -> Nexus<Input>
    // -> Nexus<Output> -> Sema<WriteInput> is pure data lowering. It
    // carries the origin route through every hop and produces the SEMA
    // write form Nexus would hand to SemaEngine::apply, all without
    // holding a &mut Store.
    let sema = SemaFile::new();
    let store = sema.open_store();
    assert!(store.is_empty(), "store starts empty");

    let signal_actor = SignalActor::default();
    let signal_input = Input::Record(entry("held in flight")).with_origin_route(route(1));
    let nexus_input = SignalEngine::triage(&signal_actor, signal_input);
    assert_eq!(nexus_input.origin_route(), route(1));
    let held_sema_input = nexus_input.into_nexus_output().into_sema_write_input();
    let plane =
        schema_meta::Plane::<Input, NexusInput, SemaWriteInput>::Sema(held_sema_input.clone());
    assert_eq!(plane.origin_route(), route(1));
    let schema_meta::Plane::Sema(sema_input) = plane else {
        panic!("expected SEMA plane");
    };
    assert_eq!(sema_input.root(), held_sema_input.root());
    match held_sema_input.root() {
        SemaWriteInput::Record(recorded) => assert_eq!(recorded.description.0, "held in flight"),
        SemaWriteInput::Remove(_) => panic!("a recorded entry lowers to a SEMA record command"),
    }

    assert!(
        store.is_empty(),
        "schema projection runs without committing to the durable SEMA store"
    );
}

#[test]
fn sema_engine_writes_durable_records_and_returns_schema_objects() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    let operation = sema_write_message(
        SemaWriteInput::Record(entry("SEMA writes durable facts")),
        1,
    );

    let response = SemaEngine::apply(&mut store, operation);

    let plane = schema_meta::Plane::<Output, NexusOutput, SemaWriteOutput>::Sema(response.clone());
    assert_eq!(plane.origin_route(), route(1));
    let schema_meta::Plane::Sema(response) = plane else {
        panic!("expected SEMA plane");
    };
    assert_eq!(response.origin_route(), route(1));
    match response.root() {
        SemaWriteOutput::Recorded(receipt) => {
            assert_eq!(receipt.record_identifier, RecordIdentifier(1));
            assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(1));
            assert_ne!(
                receipt.database_marker.state_digest,
                StateDigest(0),
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
fn nexus_engine_trait_runs_nexus_decision_through_sema_state() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input =
        NexusInput::Signal(Input::Record(entry("nexus trait root"))).with_origin_route(route(20));

    let nexus_output = NexusEngine::execute(&mut nexus, nexus_input);

    assert_eq!(nexus_output.origin_route(), route(20));
    match nexus_output.root() {
        NexusOutput::Signal(Output::RecordAccepted(receipt)) => {
            assert_eq!(receipt.record_identifier, RecordIdentifier(1));
            assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(1));
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
    let signal_actor = SignalActor::default();
    let mut nexus = Nexus::new(sema.open_store());
    let signal_input = Input::Record(entry("signal trait root")).with_origin_route(route(21));

    let nexus_input = SignalEngine::triage(&signal_actor, signal_input);
    assert_eq!(nexus_input.origin_route(), route(21));
    assert!(matches!(
        nexus_input.root(),
        NexusInput::Signal(Input::Record(_))
    ));

    let nexus_output = NexusEngine::execute(&mut nexus, nexus_input);
    let signal_output = SignalEngine::reply(&signal_actor, nexus_output);

    assert_eq!(signal_output.origin_route(), route(21));
    match signal_output.root() {
        Output::RecordAccepted(receipt) => {
            assert_eq!(receipt.record_identifier, RecordIdentifier(1));
            assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(1));
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
            sema_write_message(SemaWriteInput::Record(entry("durable one")), 1),
        );
        let recorded = SemaEngine::apply(
            &mut store,
            sema_write_message(SemaWriteInput::Record(entry("durable two")), 2),
        );
        first_marker = match recorded.into_root() {
            SemaWriteOutput::Recorded(receipt) => receipt.database_marker,
            other => panic!("expected Recorded, got {other:?}"),
        };
        assert_eq!(store.len(), 2);
        // store drops here, releasing the redb file handle.
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
        sema_write_message(SemaWriteInput::Record(entry("durable three")), 3),
    );
    match after.root() {
        SemaWriteOutput::Recorded(receipt) => {
            assert_eq!(receipt.record_identifier, RecordIdentifier(3));
            assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(3));
        }
        other => panic!("expected Recorded after reopen, got {other:?}"),
    }

    // An Observe against the reopened store finds a record written before
    // the drop, through the schema-emitted query path.
    let observed = SemaEngine::observe(
        &reopened,
        sema_read_message(SemaReadInput::Observe(query()), 4),
    );
    assert!(
        matches!(observed.root(), SemaReadOutput::Observed(_)),
        "the reopened store observes a pre-drop record"
    );
    assert_ne!(
        first_marker.state_digest,
        StateDigest(0),
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
            SemaWriteInput::Record(entry_with_topics(&["runtime-triad", "schema"], "both")),
            1,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            SemaWriteInput::Record(entry_with_topics(&["runtime-triad"], "runtime only")),
            2,
        ),
    );

    let partial = SemaEngine::observe(
        &store,
        sema_read_message(
            SemaReadInput::Observe(partial_query(&["schema", "other"], None)),
            3,
        ),
    );
    match partial.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.record_set.0.len(), 1);
            assert_eq!(records.record_set.0[0].description.0, "both");
        }
        other => panic!("expected partial query to observe one record, got {other:?}"),
    }

    let full = SemaEngine::observe(
        &store,
        sema_read_message(
            SemaReadInput::Observe(full_query(
                &["runtime-triad", "schema"],
                Some(Kind::Decision),
            )),
            4,
        ),
    );
    match full.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.record_set.0.len(), 1);
            assert_eq!(records.record_set.0[0].description.0, "both");
        }
        other => panic!("expected full query to require every topic, got {other:?}"),
    }
}

#[test]
fn sema_engine_lookup_and_count_are_read_plane_operations() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            SemaWriteInput::Record(entry_with_topics(&["runtime-triad", "schema"], "first")),
            1,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            SemaWriteInput::Record(entry_with_topics(&["runtime-triad"], "second")),
            2,
        ),
    );

    let found = SemaEngine::observe(
        &store,
        sema_read_message(SemaReadInput::Lookup(RecordIdentifier(2)), 3),
    );
    match found.root() {
        SemaReadOutput::Found(record) => {
            assert_eq!(record.record_identifier, RecordIdentifier(2));
            assert_eq!(record.entry.description.0, "second");
            assert_eq!(record.database_marker.commit_sequence, CommitSequence(2));
        }
        other => panic!("expected SEMA lookup to find the second record, got {other:?}"),
    }

    let counted = SemaEngine::observe(
        &store,
        sema_read_message(
            SemaReadInput::Count(partial_query(&["runtime-triad"], None)),
            4,
        ),
    );
    match counted.root() {
        SemaReadOutput::Counted(records) => {
            assert_eq!(records.record_count, RecordCount(2));
            assert_eq!(records.database_marker.commit_sequence, CommitSequence(2));
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
            SemaWriteInput::Record(entry_with_topics(&["runtime-triad", "schema"], "parallel")),
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
                            SemaReadInput::Observe(full_query(&["runtime-triad"], None)),
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
        sema_write_message(SemaWriteInput::Record(entry("remove target")), 1),
    );

    let removed = SemaEngine::apply(
        &mut store,
        sema_write_message(SemaWriteInput::Remove(RecordIdentifier(1)), 2),
    );
    let removed_marker = match removed.root() {
        SemaWriteOutput::Removed(receipt) => {
            assert_eq!(receipt.record_identifier, RecordIdentifier(1));
            assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(2));
            receipt.database_marker.clone()
        }
        other => panic!("expected Removed receipt, got {other:?}"),
    };

    let observed = SemaEngine::observe(
        &store,
        sema_read_message(SemaReadInput::Observe(query()), 3),
    );
    assert!(
        matches!(observed.root(), SemaReadOutput::Missed(_)),
        "removed record should not be observed again"
    );
    assert_eq!(store.len(), 0);
    assert_eq!(removed_marker.state_digest, StateDigest(0));
}

#[test]
fn nexus_runs_sema_while_holding_mail_then_replies_through_schema_objects() {
    let sema = SemaFile::new();
    let engine = sema.engine();

    let recorded = engine.handle(Input::Record(entry("nexus drives sema")));
    let plane =
        schema_meta::Plane::<Output, NexusOutput, SemaWriteOutput>::Signal(recorded.clone());
    assert_eq!(plane.origin_route(), route(1));
    let schema_meta::Plane::Signal(recorded) = plane else {
        panic!("expected Signal plane");
    };
    assert_eq!(recorded.origin_route(), route(1));
    match recorded.root() {
        Output::RecordAccepted(receipt) => {
            assert_eq!(receipt.record_identifier, RecordIdentifier(1));
            assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(1));
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    }
    assert_eq!(engine.sent_message_count(), 1);
    assert_eq!(engine.processed_message_count(), 1);
    assert_eq!(engine.record_count(), 1);
}

#[test]
fn signal_actor_rejects_invalid_input_with_schema_emitted_rejection_before_mail_or_sema() {
    let sema = SemaFile::new();
    let engine = sema.engine();
    let mut bad = entry("missing topic");
    bad.topics = Topics(vec![Topic(String::new())]);

    let output = engine.handle(Input::Record(bad));

    assert_eq!(
        output.root(),
        &Output::Rejected(SignalRejection {
            validation_error: ValidationError::EmptyTopic,
            database_marker: DatabaseMarker {
                commit_sequence: CommitSequence(0),
                state_digest: StateDigest(0),
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
fn sema_response_maps_back_to_signal_output() {
    let output = NexusInput::SemaRead(SemaReadOutput::Missed(ErrorReport {
        error_message: ErrorMessage(String::from("no matching record")),
        database_marker: DatabaseMarker {
            commit_sequence: CommitSequence(0),
            state_digest: StateDigest(0),
        },
    }))
    .with_origin_route(route(7))
    .into_nexus_output()
    .into_signal_output();

    assert_eq!(
        output.root(),
        &Output::Error(ErrorReport {
            error_message: ErrorMessage(String::from("no matching record")),
            database_marker: DatabaseMarker {
                commit_sequence: CommitSequence(0),
                state_digest: StateDigest(0),
            },
        })
    );
    assert_eq!(output.origin_route(), route(7));
}

#[test]
fn plane_envelopes_keep_payload_names_scoped() {
    let nexus_input =
        NexusInput::Signal(Input::Record(entry("language input"))).with_origin_route(route(11));
    let nexus_output = nexus_input.into_nexus_output();
    let plane =
        schema_meta::Plane::<Output, NexusOutput, SemaWriteInput>::Nexus(nexus_output.clone());
    assert_eq!(plane.origin_route(), route(11));
    let schema_meta::Plane::Nexus(nexus_output) = plane else {
        panic!("expected Nexus plane");
    };
    assert_eq!(nexus_output.origin_route(), route(11));
    assert!(matches!(
        nexus_output.root(),
        NexusOutput::SemaWrite(SemaWriteInput::Record(_))
    ));

    let sema_output = SemaWriteOutput::Recorded(SemaReceipt {
        record_identifier: RecordIdentifier(3),
        database_marker: DatabaseMarker {
            commit_sequence: CommitSequence(4),
            state_digest: StateDigest(127),
        },
    })
    .with_origin_route(route(12));
    let signal_output = sema_output
        .into_nexus_input()
        .into_nexus_output()
        .into_signal_output();
    let plane =
        schema_meta::Plane::<Output, NexusOutput, SemaWriteOutput>::Signal(signal_output.clone());
    assert_eq!(plane.origin_route(), route(12));
    let schema_meta::Plane::Signal(signal_output) = plane else {
        panic!("expected Signal plane");
    };
    assert_eq!(signal_output.origin_route(), route(12));
    assert!(matches!(signal_output.root(), Output::RecordAccepted(_)));
}

#[test]
fn nexus_and_sema_have_explicit_input_output_languages() {
    let nexus_input =
        NexusInput::Signal(Input::Record(entry("language input"))).with_origin_route(route(13));
    let nexus_output = nexus_input.into_nexus_output();
    assert!(matches!(
        nexus_output.root(),
        NexusOutput::SemaWrite(SemaWriteInput::Record(_))
    ));

    let sema_output = SemaWriteOutput::Recorded(SemaReceipt {
        record_identifier: RecordIdentifier(3),
        database_marker: DatabaseMarker {
            commit_sequence: CommitSequence(4),
            state_digest: StateDigest(127),
        },
    });
    let signal_output = NexusInput::SemaWrite(sema_output)
        .with_origin_route(route(14))
        .into_nexus_output()
        .into_signal_output();
    assert!(matches!(signal_output.root(), Output::RecordAccepted(_)));
}

#[cfg(feature = "nota-text")]
#[test]
fn import_export_paths_use_single_colon_namespaces() {
    let import = Import {
        source_path: SourcePath(String::from("signal:sema:Magnitude")),
        local_path: LocalPath(String::from("spirit:core:Magnitude")),
    };
    let export = Export {
        local_path: LocalPath(String::from("spirit:core:SemaWriteOutput")),
        public_path: PublicPath(String::from("spirit:sema:SemaWriteOutput")),
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
fn full_runtime_triad_records_then_observes_through_durable_sema() {
    let sema = SemaFile::new();
    let engine = sema.engine();

    let recorded = engine.handle(Input::Record(entry("full runtime triad works")));
    assert_eq!(recorded.origin_route(), route(1));
    let record_marker = match recorded.root() {
        Output::RecordAccepted(receipt) => {
            assert_eq!(receipt.record_identifier, RecordIdentifier(1));
            receipt.database_marker.clone()
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    };
    assert_eq!(record_marker.commit_sequence, CommitSequence(1));
    assert_eq!(engine.sent_message_count(), 1);
    assert_eq!(engine.processed_message_count(), 1);

    let observed = engine.handle(Input::Observe(Query {
        topic_match: TopicMatch::Full(Topics(vec![Topic(String::from("runtime-triad"))])),
        kind: Some(Kind::Decision),
    }));

    assert_eq!(observed.origin_route(), route(2));
    match observed.root() {
        Output::RecordsObserved(records) => {
            assert_eq!(
                records.record_set,
                RecordSet(vec![entry("full runtime triad works")])
            );
            // Observe does not advance the commit sequence; the digest
            // matches the post-record state.
            assert_eq!(records.database_marker, record_marker);
        }
        other => panic!("expected RecordsObserved, got {other:?}"),
    }
    assert_eq!(engine.sent_message_count(), 2);
    assert_eq!(engine.processed_message_count(), 2);

    assert_eq!(
        engine.mail_ledger(),
        vec![
            MailLedgerEvent::Sent(SentMail {
                mail_identifier: MailIdentifier(1),
                origin_route: route(1),
                short_header: ShortHeader(0),
            }),
            MailLedgerEvent::Processed(ProcessedMail {
                mail_identifier: MailIdentifier(1),
                origin_route: route(1),
                database_marker: record_marker.clone(),
            }),
            MailLedgerEvent::Sent(SentMail {
                mail_identifier: MailIdentifier(2),
                origin_route: route(2),
                short_header: ShortHeader(0x0001_0000_0000_0000),
            }),
            MailLedgerEvent::Processed(ProcessedMail {
                mail_identifier: MailIdentifier(2),
                origin_route: route(2),
                database_marker: record_marker,
            }),
        ]
    );
}

#[test]
fn full_runtime_triad_looks_up_and_counts_through_signal_nexus_and_sema() {
    let sema = SemaFile::new();
    let engine = sema.engine();

    engine.handle(Input::Record(entry_with_topics(
        &["runtime-triad", "schema"],
        "lookup one",
    )));
    engine.handle(Input::Record(entry_with_topics(
        &["runtime-triad"],
        "lookup two",
    )));

    let found = engine.handle(Input::Lookup(RecordIdentifier(1)));
    match found.root() {
        Output::RecordFound(record) => {
            assert_eq!(record.record_identifier, RecordIdentifier(1));
            assert_eq!(record.entry.description.0, "lookup one");
            assert_eq!(record.database_marker.commit_sequence, CommitSequence(2));
        }
        other => panic!("expected full runtime lookup to return RecordFound, got {other:?}"),
    }

    let counted = engine.handle(Input::Count(partial_query(&["runtime-triad"], None)));
    match counted.root() {
        Output::RecordsCounted(records) => {
            assert_eq!(records.record_count, RecordCount(2));
            assert_eq!(records.database_marker.commit_sequence, CommitSequence(2));
        }
        other => panic!("expected full runtime count to return RecordsCounted, got {other:?}"),
    }
}
