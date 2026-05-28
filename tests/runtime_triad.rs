use spirit_next::{
    CommitSequence, DatabaseMarker, Description, Engine, Entry, ErrorMessage, ErrorReport, Export,
    Import, Input, Kind, LocalPath, Magnitude, MailIdentifier, MailLedgerEvent, MessageIdentifier,
    MessageSent, MessageSentHook, NexusInput, NexusMail, NexusOutput, OriginRoute, Output,
    ProcessedMail, PublicPath, Query, RecordIdentifier, RecordSet, SemaEngine, SemaInput,
    SemaOutput, SemaReceipt, SentMail, ShortHeader, SignalAccepted, SignalActor, SignalRejection,
    SourcePath, StateDigest, Store, Topic, ValidationError,
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
    Entry {
        topic: Topic(String::from("runtime-triad")),
        kind: Kind::Decision,
        description: Description(String::from(description)),
        magnitude: Magnitude::Maximum,
    }
}

#[test]
fn nexus_mail_lowers_signal_payload_to_generated_sema_command() {
    let command = NexusMail::new(MessageIdentifier(1), entry("nexus mail lowers to SEMA"))
        .into_nexus_input()
        .into_nexus_output()
        .into_sema_input();

    match command {
        SemaInput::Record(recorded) => {
            assert_eq!(recorded.description.0, "nexus mail lowers to SEMA");
        }
        SemaInput::Observe(_) => panic!("record input should lower to record command"),
    }
}

#[test]
fn signal_actor_pushes_accepted_message_through_sent_hook_before_nexus_holds_mail() {
    let signal_actor = SignalActor::default();
    let signal_entry = entry("signal pushes to nexus");
    let signal_input = Input::Record(signal_entry.clone());
    let accepted = signal_actor
        .accept(signal_input.clone())
        .expect("signal input accepts");
    let mut hook = SentHookProbe { events: Vec::new() };
    let expected_sent = MailLedgerEvent::Sent(SentMail {
        mail_identifier: MailIdentifier(1),
        origin_route: OriginRoute(1),
        short_header: ShortHeader(0),
    });

    assert_eq!(accepted.message_sent().identifier, MessageIdentifier(1));
    assert_eq!(accepted.message_sent().origin_route(), OriginRoute(1));
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
fn nexus_holds_the_mail_in_being_processed_typestate_before_sema_runs() {
    // The mail's Rust TYPE is the proof: a `Mail<BeingProcessed>` exists
    // and carries the lowered SEMA input, but the durable store has not
    // yet been written. "Nexus holds the mail" is a type-level fact here:
    // only `run_sema` (which the public flow drives) produces a
    // `Mail<Processed>`.
    let sema = SemaFile::new();
    let store = sema.open_store();
    assert!(store.is_empty(), "store starts empty");

    let signal_actor = SignalActor::default();
    let accepted: SignalAccepted = signal_actor
        .accept(Input::Record(entry("held in flight")))
        .expect("signal accepts");

    let in_flight = accepted.into_being_processed();
    assert_eq!(
        in_flight.identifier(),
        MessageIdentifier(1),
        "the in-flight mail keeps the issued message identity"
    );
    assert_eq!(
        in_flight.origin_route(),
        OriginRoute(1),
        "the in-flight mail keeps the origin route return address"
    );
    match in_flight.sema_input() {
        SemaInput::Record(recorded) => assert_eq!(recorded.description.0, "held in flight"),
        SemaInput::Observe(_) => panic!("a recorded entry lowers to a SEMA record command"),
    }

    // The store is STILL empty: the mail is being processed, not yet
    // committed. The type system, not a log line, carries the phase.
    assert!(
        store.is_empty(),
        "while Nexus holds the mail in BeingProcessed, SEMA has not committed"
    );
}

#[test]
fn sema_engine_writes_durable_records_and_returns_schema_objects() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    let operation = SemaInput::Record(entry("SEMA writes durable facts"));

    let response: SemaOutput = SemaEngine::apply(&mut store, operation);

    match response {
        SemaOutput::Recorded(receipt) => {
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
fn sema_store_persists_records_across_reopen_of_the_same_sema_file() {
    // The durability proof (record 1007/1008, bead primary-q2au): write
    // records, drop the store, reopen from the same `.sema` path, and
    // observe the records survive with their commit ledger intact.
    let sema = SemaFile::new();

    let first_marker;
    {
        let mut store = sema.open_store();
        SemaEngine::apply(&mut store, SemaInput::Record(entry("durable one")));
        let recorded = SemaEngine::apply(&mut store, SemaInput::Record(entry("durable two")));
        first_marker = match recorded {
            SemaOutput::Recorded(receipt) => receipt.database_marker,
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
    let after = SemaEngine::apply(&mut reopened, SemaInput::Record(entry("durable three")));
    match after {
        SemaOutput::Recorded(receipt) => {
            assert_eq!(receipt.record_identifier, RecordIdentifier(3));
            assert_eq!(receipt.database_marker.commit_sequence, CommitSequence(3));
        }
        other => panic!("expected Recorded after reopen, got {other:?}"),
    }

    // An Observe against the reopened store finds a record written before
    // the drop, through the schema-emitted query path.
    let observed = SemaEngine::apply(
        &mut reopened,
        SemaInput::Observe(Query {
            topic: Topic(String::from("runtime-triad")),
            kind: Kind::Decision,
        }),
    );
    assert!(
        matches!(observed, SemaOutput::Observed(_)),
        "the reopened store observes a pre-drop record"
    );
    assert_ne!(
        first_marker.state_digest,
        StateDigest(0),
        "the durable digest is content-addressed, not a zero placeholder"
    );
}

#[test]
fn nexus_runs_sema_while_holding_mail_then_replies_through_schema_objects() {
    let sema = SemaFile::new();
    let engine = sema.engine();

    let recorded = engine.handle(Input::Record(entry("nexus drives sema")));
    match recorded {
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
    bad.topic = Topic(String::new());

    let output = engine.handle(Input::Record(bad));

    assert_eq!(
        output,
        Output::Rejected(SignalRejection {
            validation_error: ValidationError::EmptyTopic,
            database_marker: DatabaseMarker {
                commit_sequence: CommitSequence(0),
                state_digest: StateDigest(0),
            },
        })
    );
    assert_eq!(engine.record_count(), 0);
    assert_eq!(engine.sent_message_count(), 0);
    assert_eq!(engine.processed_message_count(), 0);
    assert_eq!(engine.mail_ledger(), []);
}

#[test]
fn sema_response_maps_back_to_signal_output() {
    let output = NexusInput::Sema(SemaOutput::Missed(ErrorReport {
        error_message: ErrorMessage(String::from("no matching record")),
        database_marker: DatabaseMarker {
            commit_sequence: CommitSequence(0),
            state_digest: StateDigest(0),
        },
    }))
    .into_nexus_output()
    .into_signal_output();

    assert_eq!(
        output,
        Output::Error(ErrorReport {
            error_message: ErrorMessage(String::from("no matching record")),
            database_marker: DatabaseMarker {
                commit_sequence: CommitSequence(0),
                state_digest: StateDigest(0),
            },
        })
    );
}

#[test]
fn nexus_and_sema_have_explicit_input_output_languages() {
    let nexus_input = NexusInput::Signal(Input::Record(entry("language input")));
    let nexus_output = nexus_input.into_nexus_output();
    assert!(matches!(
        nexus_output,
        NexusOutput::Sema(SemaInput::Record(_))
    ));

    let sema_output = SemaOutput::Recorded(SemaReceipt {
        record_identifier: RecordIdentifier(3),
        database_marker: DatabaseMarker {
            commit_sequence: CommitSequence(4),
            state_digest: StateDigest(127),
        },
    });
    let signal_output = NexusInput::Sema(sema_output)
        .into_nexus_output()
        .into_signal_output();
    assert!(matches!(signal_output, Output::RecordAccepted(_)));
}

#[test]
fn import_export_paths_use_single_colon_namespaces() {
    let import = Import {
        source_path: SourcePath(String::from("signal:sema:Magnitude")),
        local_path: LocalPath(String::from("spirit:core:Magnitude")),
    };
    let export = Export {
        local_path: LocalPath(String::from("spirit:core:SemaOutput")),
        public_path: PublicPath(String::from("spirit:sema:SemaOutput")),
    };

    assert_eq!(
        import.to_nota(),
        "([signal:sema:Magnitude] [spirit:core:Magnitude])"
    );
    assert_eq!(
        export.to_nota(),
        "([spirit:core:SemaOutput] [spirit:sema:SemaOutput])"
    );
}

#[test]
fn full_runtime_triad_records_then_observes_through_durable_sema() {
    let sema = SemaFile::new();
    let engine = sema.engine();

    let recorded = engine.handle(Input::Record(entry("full runtime triad works")));
    let record_marker = match recorded {
        Output::RecordAccepted(receipt) => {
            assert_eq!(receipt.record_identifier, RecordIdentifier(1));
            receipt.database_marker
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    };
    assert_eq!(record_marker.commit_sequence, CommitSequence(1));
    assert_eq!(engine.sent_message_count(), 1);
    assert_eq!(engine.processed_message_count(), 1);

    let observed = engine.handle(Input::Observe(Query {
        topic: Topic(String::from("runtime-triad")),
        kind: Kind::Decision,
    }));

    match observed {
        Output::RecordsObserved(records) => {
            assert_eq!(
                records.record_set,
                RecordSet(entry("full runtime triad works"))
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
                origin_route: OriginRoute(1),
                short_header: ShortHeader(0),
            }),
            MailLedgerEvent::Processed(ProcessedMail {
                mail_identifier: MailIdentifier(1),
                origin_route: OriginRoute(1),
                database_marker: record_marker.clone(),
            }),
            MailLedgerEvent::Sent(SentMail {
                mail_identifier: MailIdentifier(2),
                origin_route: OriginRoute(2),
                short_header: ShortHeader(0x0001_0000_0000_0000),
            }),
            MailLedgerEvent::Processed(ProcessedMail {
                mail_identifier: MailIdentifier(2),
                origin_route: OriginRoute(2),
                database_marker: record_marker,
            }),
        ]
    );
}
