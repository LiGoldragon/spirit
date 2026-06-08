use std::collections::HashMap;

use crate::{
    MailLedger,
    schema::{
        meta_signal::ArchiveDatabaseTarget,
        nexus::{
            self as nexus_schema, CommandSemaWrite, EngineStartFailure as NexusEngineStartFailure,
            EngineStopFailure as NexusEngineStopFailure, NexusAction, NexusEffectCommand,
            NexusEffectResult, NexusEngine, NexusWork, StashRequest, StashResult,
        },
        sema::{
            self as sema_schema, ReadInput as SemaReadInput, ReadOutput as SemaReadOutput,
            SemaEngine, WriteInput as SemaWriteInput, WriteOutput as SemaWriteOutput,
        },
        signal::{
            DatabaseMarker, Entry, ErrorReport, Input, IntentEvent, IntentRecorded,
            IntentSubscription, Kind, Magnitude, ObservedOperation, ObservedOperations,
            ObserverFilter, ObserverRetraction, ObserverSubscription, OperationKind, Output,
            Records, RemovalCandidateCollection, RemovalCandidatesCollection, SemaReceipt,
            SignalRejection, StashHandle, StashedObservation, Statement, SubscriptionToken,
            ValidationError,
        },
    },
    store::{Store, StoreError},
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, TraceEvent, TraceLog, schema::nexus::NexusObjectName};
use signal_frame::SubscriptionTokenInner;
use tokio::runtime::{Handle, RuntimeFlavor};
use triad_runtime::{ContinuationExhausted, SubscriptionTokenIssuer};

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

/// The observer-tap registry — the meta-observation surface ported from old
/// spirit's `Tap`/`Untap` operator stream.
///
/// Every admitted working operation is appended to the operation log as a typed
/// `OperationKind`. `Tap(ObserverFilter)` mints an observer subscription token,
/// records the filter, and returns the operation log filtered by that observer
/// filter so the caller sees what has been observed so far. `Untap(token)`
/// retires the subscription and returns its final filtered observations. This
/// is the request/reply half of the old observer stream: the operation history
/// is the load-bearing `OperationReceived` content, scoped by `ObserverFilter`.
#[derive(Debug, Default)]
pub struct ObserverTapTable {
    next_token: u64,
    operation_log: Vec<OperationKind>,
    taps: HashMap<u64, ObserverFilter>,
}

impl ObserverTapTable {
    /// Record one admitted operation in the observer log.
    pub fn observe_operation(&mut self, operation: OperationKind) {
        self.operation_log.push(operation);
    }

    /// Open an observer tap under a freshly minted token and return the
    /// operations observed so far, filtered by `filter`.
    pub fn open(&mut self, filter: ObserverFilter) -> (u64, ObserverFilter, ObservedOperations) {
        self.next_token += 1;
        let token = self.next_token;
        self.taps.insert(token, filter);
        (token, filter, self.observed_operations(&filter))
    }

    /// Close an observer tap. Returns the tap's final filtered observations when
    /// the token was registered, and `None` when it was not.
    pub fn close(&mut self, token: SubscriptionToken) -> Option<ObservedOperations> {
        let filter = self.taps.remove(&token)?;
        Some(self.observed_operations(&filter))
    }

    fn observed_operations(&self, filter: &ObserverFilter) -> ObservedOperations {
        self.operation_log
            .iter()
            .filter(|operation| filter.observes_operation(operation))
            .cloned()
            .map(ObservedOperation)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.taps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }
}

impl ObserverFilter {
    /// Whether this observer filter admits an operation event. `All` and
    /// `OperationsOnly` observe every operation; `EffectsOnly` observes none
    /// (effect events are not operations).
    pub fn observes_operation(&self, _operation: &OperationKind) -> bool {
        match self {
            Self::All | Self::OperationsOnly => true,
            Self::EffectsOnly => false,
        }
    }
}

impl OperationKind {
    /// The operation kind of an admitted working `Input` — the typed observer
    /// log entry recorded for the `Tap`/`Untap` surface.
    pub fn from_input(input: &Input) -> Self {
        match input {
            Input::State(_) => Self::State,
            Input::Record(_) => Self::Record,
            Input::Observe(_) => Self::Observe,
            Input::Lookup(_) => Self::Lookup,
            Input::Count(_) => Self::Count,
            Input::Remove(_) => Self::Remove,
            Input::ChangeCertainty(_) => Self::ChangeCertainty,
            Input::ChangeRecord(_) => Self::ChangeRecord,
            Input::LookupStash(_) => Self::LookupStash,
            Input::CollectRemovalCandidates(_) => Self::CollectRemovalCandidates,
            Input::Tap(_) => Self::Tap,
            Input::Untap(_) => Self::Untap,
            Input::SubscribeIntent(_) => Self::SubscribeIntent,
        }
    }
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
    observer_tap_table: ObserverTapTable,
    classification_policy: ClassificationPolicy,
    subscription_token_issuer: SubscriptionTokenIssuer,
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
            kind: self.fallback_kind,
            description: statement.into_payload(),
            magnitude: self.fallback_magnitude,
            privacy: self.fallback_privacy,
        }
    }
}

impl CommandSemaWrite {
    fn into_sema_write_input(self) -> SemaWriteInput {
        match self {
            Self::Record(record) => SemaWriteInput::record(record),
            Self::Remove(remove) => SemaWriteInput::remove(remove),
            Self::ChangeCertainty(change) => SemaWriteInput::change_certainty(change),
            Self::ChangeRecord(change) => SemaWriteInput::change_record(change),
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
                observer_tap_table: ObserverTapTable::default(),
                classification_policy: ClassificationPolicy::default(),
                subscription_token_issuer: SubscriptionTokenIssuer::default(),
            }
        }
    }

    #[cfg(feature = "testing-trace")]
    pub fn new_with_trace(store: Store, trace_log: TraceLog) -> Self {
        Self {
            store: store.with_trace(trace_log.clone()),
            mail_ledger: MailLedger::default(),
            stash_table: StashTable::default(),
            observer_tap_table: ObserverTapTable::default(),
            classification_policy: ClassificationPolicy::default(),
            subscription_token_issuer: SubscriptionTokenIssuer::default(),
            trace_log,
        }
    }

    pub fn mail_ledger(&self) -> &MailLedger {
        &self.mail_ledger
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Store the owner-configured archive target on the SEMA store. The
    /// owner-only meta `Configure` effect drives this through the same
    /// single-flight `&mut Nexus` borrow that guards every working write.
    ///
    /// This records WHERE the SEPARATE archive database lives; it does NOT open,
    /// move, or touch the live database, so the live intent log is never
    /// disturbed by a reconfigure.
    pub fn set_archive_target(&mut self, archive_target: ArchiveDatabaseTarget) {
        self.store.set_archive_target(archive_target);
    }

    pub fn archive_target(&self) -> &ArchiveDatabaseTarget {
        self.store.archive_target()
    }

    pub fn stash_table(&self) -> &StashTable {
        &self.stash_table
    }

    pub fn observer_tap_table(&self) -> &ObserverTapTable {
        &self.observer_tap_table
    }

    pub fn classification_policy(&self) -> &ClassificationPolicy {
        &self.classification_policy
    }

    pub fn database_marker(&self) -> DatabaseMarker {
        self.store.database_marker()
    }

    pub fn intent_recorded_event(
        &self,
        receipt: &SemaReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        Ok(self
            .store
            .entry_by_identifier(receipt.record_identifier)?
            .map(|entry| {
                IntentEvent::intent_recorded(IntentRecorded {
                    entry,
                    sema_receipt: receipt.clone(),
                })
            }))
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
            NexusEffectCommand::OpenIntentSubscription(_query) => {
                let token: SubscriptionTokenInner = self.subscription_token_issuer.issue();
                NexusEffectResult::intent_subscription_opened(IntentSubscription {
                    subscription_token: token.value(),
                    database_marker: self.database_marker(),
                })
            }
            NexusEffectCommand::CollectRemovalCandidates(collection) => {
                self.collect_removal_candidates(collection)
            }
            NexusEffectCommand::OpenObserverTap(filter) => {
                let (token, observer_filter, observed_operations) =
                    self.observer_tap_table.open(filter);
                NexusEffectResult::observer_tap_opened(ObserverSubscription {
                    subscription_token: token,
                    observer_filter,
                    observed_operations,
                    database_marker: self.database_marker(),
                })
            }
            NexusEffectCommand::CloseObserverTap(token) => {
                let observed_operations = self.observer_tap_table.close(token).unwrap_or_default();
                NexusEffectResult::observer_tap_closed(ObserverRetraction {
                    subscription_token: token,
                    observed_operations,
                    database_marker: self.database_marker(),
                })
            }
        }
    }

    /// Run the `CollectRemovalCandidates` working operation: archive the
    /// matching records into the SEPARATE archive database at the
    /// owner-configured target and remove them from the live log. On a store
    /// error the effect surfaces an empty collection so the caller still gets a
    /// typed reply rather than a dropped request.
    fn collect_removal_candidates(
        &mut self,
        collection: RemovalCandidateCollection,
    ) -> NexusEffectResult {
        let result = self
            .store
            .collect_removal_candidates(collection)
            .unwrap_or_else(|_error| RemovalCandidatesCollection {
                archived_records: Vec::new(),
                removed_identifiers: Vec::new(),
                skipped_removal_candidates: Vec::new(),
                database_marker: self.database_marker(),
            });
        NexusEffectResult::removal_candidates_collected(result)
    }

    /// Run a SEMA write without pinning synchronous database work onto a
    /// multi-thread async worker. Current-thread runtimes cannot use
    /// `block_in_place`, so in-process sync callers keep the direct path.
    fn apply_sema_write_operation(
        &mut self,
        input: sema_schema::sema::Sema<SemaWriteInput>,
    ) -> SemaWriteOutput {
        match Handle::current().runtime_flavor() {
            RuntimeFlavor::MultiThread => tokio::task::block_in_place(|| {
                SemaEngine::apply(&mut self.store, input).into_root()
            }),
            RuntimeFlavor::CurrentThread => SemaEngine::apply(&mut self.store, input).into_root(),
            _ => SemaEngine::apply(&mut self.store, input).into_root(),
        }
    }

    /// Run a SEMA read with the same async-runtime boundary as writes.
    fn observe_sema_read_operation(
        &self,
        input: sema_schema::sema::Sema<SemaReadInput>,
    ) -> SemaReadOutput {
        match Handle::current().runtime_flavor() {
            RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| SemaEngine::observe(&self.store, input).into_root())
            }
            RuntimeFlavor::CurrentThread => SemaEngine::observe(&self.store, input).into_root(),
            _ => SemaEngine::observe(&self.store, input).into_root(),
        }
    }
}

/// Generated `NexusEngine::execute` owns the recursive runner loop.
/// This implementation supplies the component behavior hooks: one
/// decision step, storage write/read dispatch, effect dispatch, and the
/// typed budget-exhausted reply.
impl NexusEngine for Nexus {
    fn on_start(&mut self) -> Result<(), NexusEngineStartFailure> {
        SemaEngine::on_start(&mut self.store)?;
        #[cfg(feature = "testing-trace")]
        self.trace_nexus_activation(NexusObjectName::Started);
        Ok(())
    }

    fn on_stop(&mut self) -> Result<(), NexusEngineStopFailure> {
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

    async fn apply_sema_write(
        &mut self,
        origin_route: nexus_schema::OriginRoute,
        input: CommandSemaWrite,
    ) -> SemaWriteOutput {
        self.apply_sema_write_operation(
            input
                .into_sema_write_input()
                .with_origin_route(origin_route.into()),
        )
    }

    async fn observe_sema_read(
        &mut self,
        origin_route: nexus_schema::OriginRoute,
        input: SemaReadInput,
    ) -> SemaReadOutput {
        self.observe_sema_read_operation(input.with_origin_route(origin_route.into()))
    }

    async fn run_effect(&mut self, input: NexusEffectCommand) -> NexusEffectResult {
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
    fn step_decide(&mut self, work: NexusWork) -> NexusAction {
        match work {
            NexusWork::SignalArrived(input) => self.decide_signal_arrival(input),
            NexusWork::SemaWriteCompleted(output) => self.decide_sema_write_completion(output),
            NexusWork::SemaReadCompleted(output) => self.decide_sema_read_completion(output),
            NexusWork::EffectCompleted(result) => self.decide_effect_completion(result),
        }
    }

    fn decide_signal_arrival(&mut self, input: Input) -> NexusAction {
        // Record every admitted operation in the observer log so a later
        // `Tap(ObserverFilter)` sees the operations observed so far. This is the
        // recording half of the ported `Tap`/`Untap` observer surface.
        self.observer_tap_table
            .observe_operation(OperationKind::from_input(&input));
        match input {
            Input::State(statement) => {
                NexusAction::command_effect(NexusEffectCommand::classify_state(statement))
            }
            Input::Record(record) => {
                NexusAction::command_sema_write(CommandSemaWrite::record(record))
            }
            Input::Observe(observe) => {
                NexusAction::command_sema_read(SemaReadInput::observe(observe))
            }
            Input::Lookup(lookup) => NexusAction::command_sema_read(SemaReadInput::lookup(lookup)),
            Input::Count(count) => NexusAction::command_sema_read(SemaReadInput::count(count)),
            Input::Remove(remove) => {
                NexusAction::command_sema_write(CommandSemaWrite::remove(remove))
            }
            Input::ChangeCertainty(change) => {
                NexusAction::command_sema_write(CommandSemaWrite::change_certainty(change))
            }
            Input::ChangeRecord(change) => {
                NexusAction::command_sema_write(CommandSemaWrite::change_record(change))
            }
            Input::LookupStash(handle) => match self.stash_table.lookup(&handle) {
                Some((records, database_marker)) => NexusAction::reply_to_signal(
                    Output::records_observed(crate::schema::signal::ObservedRecords {
                        record_set: records,
                        database_marker,
                    }),
                ),
                None => NexusAction::reply_to_signal(Output::rejected(SignalRejection {
                    validation_error: ValidationError::StashHandleNotFound,
                    database_marker: self.database_marker(),
                })),
            },
            Input::CollectRemovalCandidates(collection) => NexusAction::command_effect(
                NexusEffectCommand::collect_removal_candidates(collection),
            ),
            Input::Tap(filter) => {
                NexusAction::command_effect(NexusEffectCommand::open_observer_tap(filter))
            }
            Input::Untap(token) => {
                NexusAction::command_effect(NexusEffectCommand::close_observer_tap(token))
            }
            Input::SubscribeIntent(query) => {
                NexusAction::command_effect(NexusEffectCommand::open_intent_subscription(query))
            }
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
            SemaWriteOutput::CertaintyChanged(receipt) => {
                NexusAction::reply_to_signal(Output::certainty_changed(receipt))
            }
            SemaWriteOutput::RecordChanged(receipt) => {
                NexusAction::reply_to_signal(Output::record_changed(receipt))
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
                NexusAction::command_sema_write(CommandSemaWrite::record(entry))
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
            NexusEffectResult::IntentSubscriptionOpened(subscription) => {
                NexusAction::reply_to_signal(Output::subscription_started(subscription))
            }
            NexusEffectResult::RemovalCandidatesCollected(collection) => {
                NexusAction::reply_to_signal(Output::removal_candidates_collected(collection))
            }
            NexusEffectResult::ObserverTapOpened(subscription) => {
                NexusAction::reply_to_signal(Output::observation_tapped(subscription))
            }
            NexusEffectResult::ObserverTapClosed(retraction) => {
                NexusAction::reply_to_signal(Output::observation_untapped(retraction))
            }
        }
    }
}
