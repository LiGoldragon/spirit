use std::collections::HashMap;

use crate::{
    ActorStartFailure, ActorStopFailure, DatabaseMarker, ErrorReport, Input, MailLedger,
    NexusAction, NexusEffectCommand, NexusEffectResult, NexusEngine, NexusWork, Output, Records,
    SemaEngine, SemaReadInput, SemaReadOutput, SemaWriteInput, SemaWriteOutput, SignalRejection,
    StashHandle, StashRequest, StashResult, StashedObservation, ValidationError,
    schema::lib::nexus as nexus_plane, store::Store,
};

#[cfg(feature = "testing-trace")]
use crate::{NexusObjectName, ObjectName, TraceEvent, TraceLog};

/// The continuation budget per Spirit 1469: a generated-runner policy that
/// bounds how many times Nexus can recurse before the runner declares the
/// loop unsound.
///
/// Per operator 287 §"Generated Runner": *A loop that never reaches
/// `ReplyToSignal` is a runtime error, not a valid component behavior.*
/// The budget travels with the runner, not the daemon's hand-written
/// code, so every component shares the same bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContinuationBudget(u32);

impl ContinuationBudget {
    /// The default budget for spirit: 32 iterations is plenty for
    /// observe -> stash -> reply (3 iterations) plus headroom.
    pub fn default_for_pilot() -> Self {
        Self(32)
    }

    pub fn from_iteration_count(count: u32) -> Self {
        Self(count)
    }

    pub fn remaining(self) -> u32 {
        self.0
    }

    pub fn spend_one(self) -> Option<Self> {
        if self.0 == 0 {
            None
        } else {
            Some(Self(self.0 - 1))
        }
    }
}

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
/// commands, effects, recursive continuations). The runner loop drives
/// the consume → decide → act → re-consume cycle until a Signal reply
/// or the continuation budget runs out.
///
/// The pilot effect set is `Stash` only — the Observe → Stash → Reply
/// recursion proves the recursive-Nexus shape on a single real flow.
#[derive(Debug)]
pub struct Nexus {
    store: Store,
    mail_ledger: MailLedger,
    stash_table: StashTable,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
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
            }
        }
    }

    #[cfg(feature = "testing-trace")]
    pub fn new_with_trace(store: Store, trace_log: TraceLog) -> Self {
        Self {
            store: store.with_trace(trace_log.clone()),
            mail_ledger: MailLedger::default(),
            stash_table: StashTable::default(),
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

    pub fn database_marker(&self) -> DatabaseMarker {
        self.store.database_marker()
    }

    /// Apply a Nexus-local effect, producing the matching effect result
    /// that the runner re-enters as `NexusWork::EffectCompleted`.
    fn apply_effect(&mut self, command: NexusEffectCommand) -> NexusEffectResult {
        match command {
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

/// The recursive-Nexus runner loop, hand-piloted per operator 287
/// §"Generated Runner". A future schema-rust-next slice should emit this
/// loop directly from the schema; today the runner lives here so the
/// pattern can be proven on real code.
///
/// The decision plane is the hand-implemented `Nexus::decide` (NexusWork
/// → NexusAction). The runner cycles:
///
/// - `NexusAction::ReplyToSignal(reply)` → exit (the only wire exit).
/// - `NexusAction::CommandSemaWrite(command)` → SemaEngine::apply →
///   re-enter as `NexusWork::SemaWriteCompleted(reply)`.
/// - `NexusAction::CommandSemaRead(command)` → SemaEngine::observe →
///   re-enter as `NexusWork::SemaReadCompleted(reply)`.
/// - `NexusAction::CommandEffect(command)` → `apply_effect` →
///   re-enter as `NexusWork::EffectCompleted(result)`.
/// - `NexusAction::Continue(input)` → re-enter directly with the
///   nested work.
///
/// The continuation budget bounds the loop; running out is a typed
/// error reply (per Spirit 1469 + operator 287).
impl NexusEngine for Nexus {
    fn on_start(&mut self) -> Result<(), ActorStartFailure> {
        SemaEngine::on_start(&mut self.store)?;
        #[cfg(feature = "testing-trace")]
        self.trace_nexus_activation(NexusObjectName::Started);
        Ok(())
    }

    fn on_stop(&mut self) -> Result<(), ActorStopFailure> {
        #[cfg(feature = "testing-trace")]
        self.trace_nexus_activation(NexusObjectName::Stopped);
        SemaEngine::on_stop(&mut self.store)
    }

    #[cfg(feature = "testing-trace")]
    fn trace_nexus_activation(&self, object_name: NexusObjectName) {
        self.trace_log
            .record(TraceEvent::new(ObjectName::Nexus(object_name)));
    }

    fn decide(
        &mut self,
        input: nexus_plane::Nexus<nexus_plane::Work>,
    ) -> nexus_plane::Nexus<nexus_plane::Action> {
        let origin_route = input.origin_route();
        let mut work = input.into_root();
        let mut budget = ContinuationBudget::default_for_pilot();

        loop {
            // Step the engine: NexusWork → NexusAction.
            let action = self.step_decide(work);

            match action {
                NexusAction::ReplyToSignal(reply) => {
                    return NexusAction::reply_to_signal(reply).with_origin_route(origin_route);
                }
                NexusAction::CommandSemaWrite(command) => {
                    let sema_output =
                        SemaEngine::apply(&mut self.store, command.with_origin_route(origin_route));
                    work = NexusWork::sema_write_completed(sema_output.into_root());
                }
                NexusAction::CommandSemaRead(command) => {
                    let sema_output =
                        SemaEngine::observe(&self.store, command.with_origin_route(origin_route));
                    work = NexusWork::sema_read_completed(sema_output.into_root());
                }
                NexusAction::CommandEffect(command) => {
                    let result = self.apply_effect(command);
                    work = NexusWork::effect_completed(result);
                }
                NexusAction::Continue(next_work) => {
                    work = next_work;
                }
            }

            match budget.spend_one() {
                Some(remaining) => budget = remaining,
                None => {
                    return NexusAction::reply_to_signal(Output::error(ErrorReport {
                        error_message: String::from("nexus continuation budget exhausted"),
                        database_marker: self.database_marker(),
                    }))
                    .with_origin_route(origin_route);
                }
            }
        }
    }
}

impl Nexus {
    /// One step of the decision plane: consume a NexusWork, emit a
    /// NexusAction. The runner loop above drives multiple steps.
    ///
    /// The Observe-with-Stash flow lives here: a SemaRead completion
    /// with non-empty results becomes a `CommandEffect(Stash(...))`
    /// recursion (NOT a direct Signal reply), and the EffectCompleted
    /// (Stashed) feedback becomes the slim `Output::RecordsStashed`.
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
