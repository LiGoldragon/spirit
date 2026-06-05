use std::collections::HashMap;

use crate::{
    DatabaseMarker, Entry, ErrorReport, Input, Kind, Magnitude, MailLedger, NexusAction,
    NexusActorStartFailure, NexusActorStopFailure, NexusEffectCommand, NexusEffectResult,
    NexusEngine, NexusWork, Output, Records, SemaEngine, SemaReadInput, SemaReadOutput,
    SemaWriteInput, SemaWriteOutput, SignalRejection, StashHandle, StashRequest, StashResult,
    StashedObservation, Statement, ValidationError, schema::nexus as nexus_schema, store::Store,
};

#[cfg(feature = "testing-trace")]
use crate::{NexusObjectName, ObjectName, TraceEvent, TraceLog};
use triad_runtime::ContinuationExhausted;

/// The stash table — the durable handle store backing the Stash effect.
///
/// The full-records observation gets archived under a freshly minted
/// `StashHandle`; the slim reply carries the handle + the record count.
/// A follow-up `Input::LookupStash(handle)` returns the full records as
/// a normal `RecordsObserved` output.
#[derive(Debug, Default)]
pub struct StashTable {
    next_handle: u64,
    entries: HashMap<u64, StashEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassificationPolicy {
    fallback_topic: String,
    fallback_kind: Kind,
    fallback_magnitude: Magnitude,
    fallback_privacy: Magnitude,
}

#[derive(Clone, Debug)]
struct StashEntry {
    records: Records,
    database_marker: DatabaseMarker,
}

impl StashTable {
    /// Mint a fresh handle and archive the records.
    pub fn put(&mut self, records: Records, database_marker: DatabaseMarker) -> StashResult {
        self.next_handle += 1;
        let handle = self.next_handle;
        let record_count = records.len() as u64;
        self.entries.insert(
            handle,
            StashEntry {
                records,
                database_marker: database_marker.clone(),
            },
        );
        StashResult {
            stash_handle: handle,
            record_count,
            database_marker,
        }
    }

    /// Look up records by handle. Returns the archived records plus the
    /// marker the stash was sealed under.
    pub fn lookup(&self, handle: &StashHandle) -> Option<(Records, DatabaseMarker)> {
        self.entries
            .get(handle)
            .map(|entry| (entry.records.clone(), entry.database_marker.clone()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Nexus is the runtime decision center between Signal and SEMA.
///
/// Per Spirit 1438 + 1439 (and operator 287 §"Recursive Computation"):
/// Nexus consumes typed `NexusWork` (facts: SignalArrived, completion
/// events) and emits typed `NexusAction` (actions: replies, SEMA
/// commands, effects, recursive continuations). Generated `NexusEngine`
/// glue and `triad-runtime::Runner` drive the consume → decide → act →
/// re-consume cycle until a Signal reply or the continuation budget runs
/// out.
///
/// The pilot effect set keeps internal features visible in schema:
/// `Stash` exposes the Observe → Stash → Reply recursion, and
/// `ClassifyState` exposes State classification before the resulting
/// Entry is written through SEMA.
#[derive(Debug)]
pub struct Nexus {
    store: Store,
    mail_ledger: MailLedger,
    stash_table: StashTable,
    classification_policy: ClassificationPolicy,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
}

impl Default for ClassificationPolicy {
    fn default() -> Self {
        Self {
            fallback_topic: String::from("unclassified"),
            fallback_kind: Kind::Clarification,
            fallback_magnitude: Magnitude::Minimum,
            fallback_privacy: Magnitude::Zero,
        }
    }
}

impl ClassificationPolicy {
    pub fn classify(&self, statement: Statement) -> Entry {
        Entry {
            topics: vec![self.fallback_topic.clone()],
            kind: self.fallback_kind.clone(),
            description: statement.into_payload(),
            magnitude: self.fallback_magnitude.clone(),
            privacy: self.fallback_privacy.clone(),
        }
    }
}

impl Nexus {
    /// Build a Nexus over a durable SEMA store and a fresh mail ledger.
    pub fn new(store: Store) -> Self {
        #[cfg(feature = "testing-trace")]
        {
            Self::new_with_trace(store, TraceLog::default())
        }
        #[cfg(not(feature = "testing-trace"))]
        {
            Self {
                store,
                mail_ledger: MailLedger::default(),
                stash_table: StashTable::default(),
                classification_policy: ClassificationPolicy::default(),
            }
        }
    }

    #[cfg(feature = "testing-trace")]
    pub fn new_with_trace(store: Store, trace_log: TraceLog) -> Self {
        Self {
            store: store.with_trace(trace_log.clone()),
            mail_ledger: MailLedger::default(),
            stash_table: StashTable::default(),
            classification_policy: ClassificationPolicy::default(),
            trace_log,
        }
    }

    pub fn mail_ledger(&self) -> &MailLedger {
        &self.mail_ledger
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn stash_table(&self) -> &StashTable {
        &self.stash_table
    }

    pub fn classification_policy(&self) -> &ClassificationPolicy {
        &self.classification_policy
    }

    pub fn database_marker(&self) -> DatabaseMarker {
        self.store.database_marker()
    }

    /// Apply a Nexus-local effect, producing the matching effect result
    /// that the runner re-enters as `NexusWork::EffectCompleted`.
    fn apply_effect(&mut self, command: NexusEffectCommand) -> NexusEffectResult {
        match command {
            NexusEffectCommand::ClassifyState(statement) => {
                let entry = self.classification_policy.classify(statement);
                NexusEffectResult::state_classified(entry)
            }
            NexusEffectCommand::Stash(StashRequest {
                records,
                database_marker,
            }) => {
                let result = self.stash_table.put(records, database_marker);
                NexusEffectResult::stashed(result)
            }
        }
    }
}

/// Generated `NexusEngine::execute` owns the recursive runner loop.
/// This implementation supplies the component behavior hooks: one
/// decision step, storage write/read dispatch, effect dispatch, and the
/// typed budget-exhausted reply.
impl NexusEngine for Nexus {
    fn on_start(&mut self) -> Result<(), NexusActorStartFailure> {
        SemaEngine::on_start(&mut self.store)?;
        #[cfg(feature = "testing-trace")]
        self.trace_nexus_activation(NexusObjectName::Started);
        Ok(())
    }

    fn on_stop(&mut self) -> Result<(), NexusActorStopFailure> {
        #[cfg(feature = "testing-trace")]
        self.trace_nexus_activation(NexusObjectName::Stopped);
        SemaEngine::on_stop(&mut self.store)?;
        Ok(())
    }

    #[cfg(feature = "testing-trace")]
    fn trace_nexus_activation(&self, object_name: NexusObjectName) {
        self.trace_log
            .record(TraceEvent::new(ObjectName::Nexus(object_name)));
    }

    fn decide(
        &mut self,
        input: nexus_schema::nexus::Nexus<nexus_schema::nexus::Work>,
    ) -> nexus_schema::nexus::Nexus<nexus_schema::nexus::Action> {
        let origin_route = input.origin_route();
        self.step_decide(input.into_root())
            .with_origin_route(origin_route)
    }

    fn apply_sema_write(
        &mut self,
        origin_route: nexus_schema::OriginRoute,
        input: SemaWriteInput,
    ) -> SemaWriteOutput {
        SemaEngine::apply(
            &mut self.store,
            input.with_origin_route(origin_route.into()),
        )
        .into_root()
    }

    fn observe_sema_read(
        &self,
        origin_route: nexus_schema::OriginRoute,
        input: SemaReadInput,
    ) -> SemaReadOutput {
        SemaEngine::observe(&self.store, input.with_origin_route(origin_route.into())).into_root()
    }

    fn run_effect(&mut self, input: NexusEffectCommand) -> NexusEffectResult {
        self.apply_effect(input)
    }

    fn budget_exhausted_reply(&self, exhausted: ContinuationExhausted) -> Output {
        Output::error(ErrorReport {
            error_message: format!(
                "nexus continuation budget exhausted after {} steps (limit {})",
                exhausted.completed_step_count(),
                exhausted.limit().count()
            ),
            database_marker: self.database_marker(),
        })
    }
}

impl Nexus {
    /// One step of the decision plane: consume a NexusWork, emit a
    /// NexusAction. Generated `NexusEngine::execute` drives multiple
    /// steps through `triad-runtime::Runner`.
    ///
    /// The Observe-with-Stash flow lives here: a SemaRead completion
    /// with non-empty results becomes a `CommandEffect(Stash(...))`
    /// recursion (NOT a direct Signal reply), and the EffectCompleted
    /// (Stashed) feedback becomes the slim `Output::RecordsStashed`.
    /// State classification also lives here as a schema-declared
    /// `CommandEffect(ClassifyState)` followed by
    /// `EffectCompleted(StateClassified)` and the ordinary SEMA
    /// `Record` write.
    fn step_decide(&self, work: NexusWork) -> NexusAction {
        match work {
            NexusWork::SignalArrived(input) => self.decide_signal_arrival(input),
            NexusWork::SemaWriteCompleted(output) => self.decide_sema_write_completion(output),
            NexusWork::SemaReadCompleted(output) => self.decide_sema_read_completion(output),
            NexusWork::EffectCompleted(result) => self.decide_effect_completion(result),
        }
    }

    fn decide_signal_arrival(&self, input: Input) -> NexusAction {
        match input {
            Input::State(statement) => {
                NexusAction::command_effect(NexusEffectCommand::classify_state(statement))
            }
            Input::Record(record) => {
                NexusAction::command_sema_write(SemaWriteInput::record(record))
            }
            Input::Observe(observe) => {
                NexusAction::command_sema_read(SemaReadInput::observe(observe))
            }
            Input::Lookup(lookup) => NexusAction::command_sema_read(SemaReadInput::lookup(lookup)),
            Input::Count(count) => NexusAction::command_sema_read(SemaReadInput::count(count)),
            Input::Remove(remove) => {
                NexusAction::command_sema_write(SemaWriteInput::remove(remove))
            }
            Input::LookupStash(handle) => match self.stash_table.lookup(&handle) {
                Some((records, database_marker)) => {
                    NexusAction::reply_to_signal(Output::records_observed(crate::ObservedRecords {
                        record_set: records,
                        database_marker,
                    }))
                }
                None => NexusAction::reply_to_signal(Output::rejected(SignalRejection {
                    validation_error: ValidationError::StashHandleNotFound,
                    database_marker: self.database_marker(),
                })),
            },
        }
    }

    fn decide_sema_write_completion(&self, output: SemaWriteOutput) -> NexusAction {
        match output {
            SemaWriteOutput::Recorded(receipt) => {
                NexusAction::reply_to_signal(Output::record_accepted(receipt))
            }
            SemaWriteOutput::Removed(receipt) => {
                NexusAction::reply_to_signal(Output::record_removed(receipt))
            }
            SemaWriteOutput::Missed(report) => NexusAction::reply_to_signal(Output::error(report)),
        }
    }

    fn decide_sema_read_completion(&self, output: SemaReadOutput) -> NexusAction {
        match output {
            SemaReadOutput::Observed(observed) => {
                // Observe's slim-output path per Spirit 1389: recurse
                // through Stash effect so the wire reply carries a
                // handle, not the full record set.
                let database_marker = observed.database_marker;
                let records = observed.record_set;
                NexusAction::command_effect(NexusEffectCommand::stash(StashRequest {
                    records,
                    database_marker,
                }))
            }
            SemaReadOutput::Found(record) => {
                NexusAction::reply_to_signal(Output::record_found(record))
            }
            SemaReadOutput::Counted(counted) => {
                NexusAction::reply_to_signal(Output::records_counted(counted))
            }
            SemaReadOutput::Missed(report) => NexusAction::reply_to_signal(Output::error(report)),
        }
    }

    fn decide_effect_completion(&self, result: NexusEffectResult) -> NexusAction {
        match result {
            NexusEffectResult::StateClassified(entry) => {
                NexusAction::command_sema_write(SemaWriteInput::record(entry))
            }
            NexusEffectResult::Stashed(StashResult {
                stash_handle,
                record_count,
                database_marker,
            }) => NexusAction::reply_to_signal(Output::records_stashed(StashedObservation {
                stash_handle,
                record_count,
                database_marker,
            })),
        }
    }
}
