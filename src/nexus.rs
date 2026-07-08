use std::collections::HashMap;

#[cfg(feature = "agent-guardian")]
use crate::guardian_journal::GuardianOperation;

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
            Aliases, ApplyRefusal, ApplyRefusalReason, Certainty, Clarification,
            ClarificationReceipt, ClarificationResolution, ClarificationResolutionReceipt,
            DatabaseMarker, Description, Domain, Domains, Entry, ErrorMessage, ErrorReport,
            GuardianRejection, Importance, Input, IntentClarifiedEvent, IntentEvent,
            IntentRecordedEvent, IntentSubscription, IntentSupersededEvent, Justification, Kind,
            Magnitude, ObservedOperation, ObservedOperations, ObservedRecords, ObserverFilter,
            ObserverRetraction, ObserverSubscription, OperationKind, Output, Privacy, Proposal,
            QuoteText, Reasoning, RecordChange, RecordChangeReceipt, RecordCount, RecordIdentifier,
            RecordRequest, RecordSet, Records, Referent, ReferentGuardianRejection,
            ReferentRegistration, ReferentRegistrationReceipt, Referents, Replacements, Retirement,
            RetirementReceipt, SemaReceipt, SignalRejection, StashHandle, StashedObservation,
            Statement, SubscriptionToken, Supersession, SupersessionReceipt, Testimony,
            ValidationError, VerbatimQuote, VersionReport, VersionText,
        },
    },
    store::{Store, StoreError},
};

#[cfg(feature = "agent-guardian")]
use crate::schema::signal::{
    Explanation, GuardianRejectionReason, ReferentGuardianRejectionReason,
};

#[cfg(feature = "testing-trace")]
use crate::{ObjectName, TraceEvent, TraceLog, schema::nexus::NexusObjectName};
use signal_frame::SubscriptionTokenInner;
use tokio::runtime::{Handle, RuntimeFlavor};
use triad_runtime::{ContinuationExhausted, ContinuationLimit, SubscriptionTokenIssuer};

/// The stash table — the durable handle store backing the Stash effect.
///
/// The full-records observation gets archived under a freshly minted
/// `StashHandle`; the reply carries the handle, count, and records together.
/// A follow-up `Input::LookupStash(handle)` recovers the same records as a
/// normal `RecordsObserved` output.
#[derive(Debug, Default)]
pub struct StashTable {
    next_handle: u64,
    entries: HashMap<u64, StashEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassificationPolicy {
    fallback_domain: Domain,
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
        let filter = self.taps.remove(token.payload())?;
        Some(self.observed_operations(&filter))
    }

    fn observed_operations(&self, filter: &ObserverFilter) -> ObservedOperations {
        ObservedOperations::new(
            self.operation_log
                .iter()
                .filter(|operation| filter.observes_operation(operation))
                .cloned()
                .map(ObservedOperation::new)
                .collect(),
        )
    }

    pub fn len(&self) -> usize {
        self.taps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
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
        let record_count = records.payload().len() as u64;
        let observed_records = records.clone();
        self.entries.insert(
            handle,
            StashEntry {
                records,
                database_marker: database_marker.clone(),
            },
        );
        StashResult {
            stash_handle: StashHandle::new(handle),
            record_count: RecordCount::new(record_count),
            database_marker,
            records: observed_records,
        }
    }

    /// Look up records by handle. Returns the archived records plus the
    /// marker the stash was sealed under.
    pub fn lookup(&self, handle: &StashHandle) -> Option<(Records, DatabaseMarker)> {
        self.entries
            .get(handle.payload())
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
    #[cfg(feature = "agent-guardian")]
    guardian: Option<crate::guardian::AgentGuardian>,
    #[cfg(feature = "agent-guardian")]
    guardian_required: bool,
    #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
    operation_authorizer: crate::criome_gate::SpiritOperationAuthorizer,
    #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
    operation_authorization_mode: signal_spirit::AuthorizationMode,
    subscription_token_issuer: SubscriptionTokenIssuer,
    #[cfg(feature = "testing-trace")]
    trace_log: TraceLog,
}

impl Default for ClassificationPolicy {
    fn default() -> Self {
        Self {
            fallback_domain: Domain::Information(crate::schema::signal::Information::Documentation),
            fallback_kind: Kind::Clarification,
            fallback_magnitude: Magnitude::Minimum,
            fallback_privacy: Magnitude::Zero,
        }
    }
}

impl ClassificationPolicy {
    pub fn classify(&self, statement: Statement) -> RecordRequest {
        let statement_text = statement.into_payload();
        let description = statement_text.payload().clone();
        let justification = Justification {
            testimony: Testimony::new(vec![VerbatimQuote::new(
                QuoteText::new(description.clone()),
                None,
            )]),
            reasoning: Reasoning::new(description.clone()),
        };
        let entry = Entry {
            domains: Domains::new(vec![self.fallback_domain.clone()]),
            kind: self.fallback_kind,
            description: Description::new(description),
            certainty: Certainty::new(self.fallback_magnitude),
            importance: Importance::new(Magnitude::Minimum),
            privacy: Privacy::new(self.fallback_privacy),
            referents: Referents::new(vec![Referent::new("state")]),
        };
        RecordRequest {
            entry,
            justification,
        }
    }
}

impl CommandSemaWrite {
    fn into_sema_write_input(self) -> SemaWriteInput {
        match self {
            Self::Record(record) => SemaWriteInput::record(record),
            Self::ChangeCertainty(change) => SemaWriteInput::change_certainty(change),
            Self::BumpImportance(change) => SemaWriteInput::bump_importance(change),
            Self::ChangeRecord(change) => SemaWriteInput::change_record(change),
            Self::RegisterReferent(register) => SemaWriteInput::register_referent(register),
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
                #[cfg(feature = "agent-guardian")]
                guardian: None,
                #[cfg(feature = "agent-guardian")]
                guardian_required: false,
                #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
                operation_authorizer: crate::criome_gate::SpiritOperationAuthorizer::new(),
                #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
                operation_authorization_mode: signal_spirit::AuthorizationMode::Gating,
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
            #[cfg(feature = "agent-guardian")]
            guardian: None,
            #[cfg(feature = "agent-guardian")]
            guardian_required: false,
            #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
            operation_authorizer: crate::criome_gate::SpiritOperationAuthorizer::new(),
            #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
            operation_authorization_mode: signal_spirit::AuthorizationMode::Gating,
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

    #[cfg(feature = "agent-guardian")]
    pub fn set_guardian(&mut self, guardian: crate::guardian::AgentGuardian) {
        self.guardian = Some(guardian);
        self.guardian_required = true;
    }

    #[cfg(feature = "agent-guardian")]
    pub fn require_guardian(&mut self) {
        self.guardian_required = true;
    }

    /// Apply an owner-supplied guardian prompt source to the live guardian. A
    /// no-op when no guardian is installed (an owner can still configure a
    /// prompt for a daemon whose guardian is wired in later — the installed
    /// guardian always starts on the compiled-in default, and the next
    /// `Configure` re-applies the override). Used by the meta `Configure` path.
    #[cfg(feature = "agent-guardian")]
    pub fn set_guardian_prompt_source(
        &mut self,
        prompt_source: crate::guardian_prompt::GuardianPromptSource,
    ) {
        if let Some(guardian) = self.guardian.as_mut() {
            guardian.set_prompt_source(prompt_source);
        }
    }

    /// The intent-guardian system prompt the installed guardian will currently
    /// send, or `None` when no guardian is installed. Diagnostic over the live
    /// prompt source.
    #[cfg(feature = "agent-guardian")]
    pub fn guardian_intent_system_prompt(&self) -> Option<String> {
        self.guardian
            .as_ref()
            .map(|guardian| guardian.intent_guardian_system_prompt())
    }

    #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
    pub fn set_operation_authorization_mode(
        &mut self,
        authorization_mode: signal_spirit::AuthorizationMode,
    ) {
        self.operation_authorization_mode = authorization_mode;
    }

    #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
    pub fn configure_operation_authorizer(&mut self, socket: impl Into<std::path::PathBuf>) {
        self.operation_authorizer.configure_socket(socket);
    }

    #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
    pub fn clear_operation_authorizer(&mut self) {
        self.operation_authorizer.clear();
    }

    pub fn intent_recorded_event(
        &self,
        record_identifier: &RecordIdentifier,
    ) -> Result<Option<IntentEvent>, StoreError> {
        Ok(self
            .store
            .entry_by_identifier(record_identifier.payload())?
            .map(|entry| {
                IntentEvent::intent_recorded(IntentRecordedEvent {
                    entry,
                    record_identifier: record_identifier.clone(),
                })
            }))
    }

    pub fn intent_clarified_event(
        &self,
        receipt: &ClarificationReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        Ok(self
            .store
            .entry_by_identifier(receipt.payload().payload())?
            .map(|entry| {
                IntentEvent::intent_clarified(IntentClarifiedEvent {
                    record_identifier: receipt.payload().clone(),
                    entry,
                })
            }))
    }

    pub fn intent_superseded_event(
        &self,
        receipt: &SupersessionReceipt,
    ) -> Result<Option<IntentEvent>, StoreError> {
        // Resolve the replacement entries for the event. A missing lookup must
        // not silently drop the whole retirement notification, so skip a missing
        // one rather than collapsing to None (under single-flight every id
        // resolves; this only hardens against a future concurrency relaxation).
        let mut replacements = Vec::new();
        for identifier in receipt.record_identifiers.payload() {
            if let Some(entry) = self.store.entry_by_identifier(identifier.payload())? {
                replacements.push(entry);
            }
        }
        Ok(Some(IntentEvent::intent_superseded(
            IntentSupersededEvent {
                retired_identifiers: receipt.retired_identifiers.clone(),
                replacements: Replacements::new(replacements),
                record_identifiers: receipt.record_identifiers.clone(),
            },
        )))
    }

    pub fn intent_retired_event(&self, receipt: &RetirementReceipt) -> IntentEvent {
        IntentEvent::intent_retired(receipt.payload().clone())
    }

    /// Apply a Nexus-local effect, producing the matching effect result
    /// that the runner re-enters as `NexusWork::EffectCompleted`.
    async fn apply_effect(&mut self, command: NexusEffectCommand) -> NexusEffectResult {
        match command {
            NexusEffectCommand::ClassifyState(statement) => {
                let record_request = self.classification_policy.classify(statement);
                NexusEffectResult::state_classified(record_request)
            }
            NexusEffectCommand::RecordWithImpliedReferents(record) => {
                self.apply_record_with_implied_referents(record).await
            }
            NexusEffectCommand::GuardRecord(record) => match self.guard_record(record).await {
                Ok(Ok(receipt)) => {
                    #[cfg(feature = "testing-trace")]
                    self.trace_direct_sema_write();
                    NexusEffectResult::recorded(receipt)
                }
                Ok(Err(rejection)) => NexusEffectResult::guardian_rejected(rejection),
                Err(error) => self.operation_failed(error.to_string()),
            },
            NexusEffectCommand::ProposeWithImpliedReferents(propose) => {
                self.apply_propose_with_implied_referents(propose).await
            }
            NexusEffectCommand::Propose(propose) => match self.guard_propose(propose).await {
                Ok(Ok(receipt)) => {
                    #[cfg(feature = "testing-trace")]
                    self.trace_direct_sema_write();
                    NexusEffectResult::proposed(receipt)
                }
                Ok(Err(rejection)) => NexusEffectResult::guardian_rejected(rejection),
                Err(error) => self.operation_failed(error.to_string()),
            },
            NexusEffectCommand::Clarify(clarify) => match self.guard_clarify(clarify).await {
                Ok(Ok(Some(receipt))) => {
                    #[cfg(feature = "testing-trace")]
                    self.trace_direct_sema_write();
                    NexusEffectResult::clarified(receipt)
                }
                Ok(Ok(None)) => self.operation_failed("record not found"),
                Ok(Err(rejection)) => NexusEffectResult::guardian_rejected(rejection),
                Err(error) => self.operation_failed(error.to_string()),
            },
            NexusEffectCommand::ResolveClarification(resolution) => {
                match self.guard_resolve_clarification(resolution).await {
                    Ok(Ok(Some(receipt))) => {
                        #[cfg(feature = "testing-trace")]
                        self.trace_direct_sema_write();
                        NexusEffectResult::clarification_resolved(receipt)
                    }
                    Ok(Ok(None)) => {
                        self.operation_failed("clarification resolution target not found")
                    }
                    Ok(Err(rejection)) => NexusEffectResult::guardian_rejected(rejection),
                    Err(error) => self.operation_failed(error.to_string()),
                }
            }
            NexusEffectCommand::SupersedeWithImpliedReferents(supersede) => {
                self.apply_supersede_with_implied_referents(supersede).await
            }
            NexusEffectCommand::Supersede(supersede) => {
                match self.guard_supersede(supersede).await {
                    Ok(Ok(Some(receipt))) => {
                        #[cfg(feature = "testing-trace")]
                        self.trace_direct_sema_write();
                        NexusEffectResult::superseded(receipt)
                    }
                    Ok(Ok(None)) => self.operation_failed("supersede target not found"),
                    Ok(Err(rejection)) => NexusEffectResult::guardian_rejected(rejection),
                    Err(error) => self.operation_failed(error.to_string()),
                }
            }
            NexusEffectCommand::Retire(retire) => match self.guard_retire(retire).await {
                Ok(Ok(Some(receipt))) => {
                    #[cfg(feature = "testing-trace")]
                    self.trace_direct_sema_write();
                    NexusEffectResult::retired(receipt)
                }
                Ok(Ok(None)) => self.operation_failed("record not found"),
                Ok(Err(rejection)) => NexusEffectResult::guardian_rejected(rejection),
                Err(error) => self.operation_failed(error.to_string()),
            },
            NexusEffectCommand::ChangeRecordWithImpliedReferents(change) => {
                self.apply_change_record_with_implied_referents(change)
                    .await
            }
            NexusEffectCommand::GuardChangeRecord(change) => {
                match self.guard_change_record(change).await {
                    Ok(Ok(Some(receipt))) => {
                        #[cfg(feature = "testing-trace")]
                        self.trace_direct_sema_write();
                        NexusEffectResult::record_changed(receipt)
                    }
                    Ok(Ok(None)) => self.operation_failed("record not found"),
                    Ok(Err(rejection)) => NexusEffectResult::guardian_rejected(rejection),
                    Err(error) => self.operation_failed(error.to_string()),
                }
            }
            NexusEffectCommand::GuardReferentRegistration(register) => {
                match self.guard_referent_registration(register).await {
                    Ok(Ok(receipt)) => NexusEffectResult::referent_registered(receipt),
                    Ok(Err(rejection)) => NexusEffectResult::referent_guardian_rejected(rejection),
                    Err(error) => self.operation_failed(error.to_string()),
                }
            }
            NexusEffectCommand::Stash(stash) => {
                let StashRequest {
                    records,
                    database_marker,
                } = stash;
                let result = self.stash_table.put(records, database_marker);
                NexusEffectResult::stashed(result)
            }
            NexusEffectCommand::OpenIntentSubscription(_query) => {
                let token: SubscriptionTokenInner = self.subscription_token_issuer.issue();
                NexusEffectResult::intent_subscription_opened(IntentSubscription::new(
                    SubscriptionToken::new(token.value()),
                ))
            }
            NexusEffectCommand::OpenObserverTap(filter) => {
                let (token, observer_filter, observed_operations) =
                    self.observer_tap_table.open(filter);
                NexusEffectResult::observer_tap_opened(ObserverSubscription {
                    subscription_token: SubscriptionToken::new(token),
                    observer_filter,
                    observed_operations,
                })
            }
            NexusEffectCommand::CloseObserverTap(subscription_token) => {
                let observed_operations = self
                    .observer_tap_table
                    .close(subscription_token.clone())
                    .unwrap_or_else(|| ObservedOperations::new(Vec::new()));
                NexusEffectResult::observer_tap_closed(ObserverRetraction {
                    subscription_token,
                    observed_operations,
                })
            }
        }
    }

    async fn apply_record_with_implied_referents(
        &mut self,
        request: RecordRequest,
    ) -> NexusEffectResult {
        match self
            .register_implied_referents(&request.entry, &request.justification)
            .await
        {
            Ok(Ok(())) => NexusEffectResult::record_referents_settled(request),
            Ok(Err(rejection)) => NexusEffectResult::referent_guardian_rejected(rejection),
            Err(error) => self.operation_failed(error.to_string()),
        }
    }

    async fn apply_propose_with_implied_referents(
        &mut self,
        proposal: Proposal,
    ) -> NexusEffectResult {
        match self
            .register_implied_referents(&proposal.entry, &proposal.justification)
            .await
        {
            Ok(Ok(())) => NexusEffectResult::propose_referents_settled(proposal),
            Ok(Err(rejection)) => NexusEffectResult::referent_guardian_rejected(rejection),
            Err(error) => self.operation_failed(error.to_string()),
        }
    }

    async fn apply_supersede_with_implied_referents(
        &mut self,
        supersession: Supersession,
    ) -> NexusEffectResult {
        for replacement in supersession.replacements.payload().clone() {
            match self
                .register_implied_referents(&replacement, &supersession.justification)
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(rejection)) => {
                    return NexusEffectResult::referent_guardian_rejected(rejection);
                }
                Err(error) => return self.operation_failed(error.to_string()),
            }
        }
        NexusEffectResult::supersede_referents_settled(supersession)
    }

    async fn apply_change_record_with_implied_referents(
        &mut self,
        change: RecordChange,
    ) -> NexusEffectResult {
        match self
            .register_implied_referents(&change.entry, &change.justification)
            .await
        {
            Ok(Ok(())) => NexusEffectResult::change_record_referents_settled(change),
            Ok(Err(rejection)) => NexusEffectResult::referent_guardian_rejected(rejection),
            Err(error) => self.operation_failed(error.to_string()),
        }
    }

    #[cfg(not(feature = "agent-guardian"))]
    async fn guard_record(
        &mut self,
        request: RecordRequest,
    ) -> Result<Result<SemaReceipt, GuardianRejection>, StoreError> {
        Ok(Ok(self.store.record_entry(request.entry)?))
    }

    #[cfg(feature = "agent-guardian")]
    async fn guard_record(
        &mut self,
        request: RecordRequest,
    ) -> Result<Result<SemaReceipt, GuardianRejection>, StoreError> {
        let entry = request.entry.clone();
        let operation = GuardianOperation::record(request);
        let authorization_operation = operation.clone();
        if let Some(rejection) = self.guard_model(operation)? {
            return Ok(Err(self.duplicate_rejection_if_needed(&entry, rejection)?));
        }
        self.authorize_guardian_operation(&authorization_operation)
            .await?;
        Ok(Ok(self.store.record_entry(entry)?))
    }

    #[cfg(not(feature = "agent-guardian"))]
    async fn guard_propose(
        &mut self,
        proposal: Proposal,
    ) -> Result<Result<SemaReceipt, GuardianRejection>, StoreError> {
        self.store.guard_propose(proposal.entry)
    }

    #[cfg(feature = "agent-guardian")]
    async fn guard_propose(
        &mut self,
        proposal: Proposal,
    ) -> Result<Result<SemaReceipt, GuardianRejection>, StoreError> {
        if self.guardian.is_none() {
            return self.store.guard_propose(proposal.entry);
        }
        let entry = proposal.entry.clone();
        let operation = GuardianOperation::propose(proposal);
        let authorization_operation = operation.clone();
        if let Some(rejection) = self.guard_model(operation)? {
            return Ok(Err(self.duplicate_rejection_if_needed(&entry, rejection)?));
        }
        self.authorize_guardian_operation(&authorization_operation)
            .await?;
        Ok(Ok(self.store.propose(entry)?))
    }

    #[cfg(not(feature = "agent-guardian"))]
    async fn guard_clarify(
        &mut self,
        clarification: Clarification,
    ) -> Result<Result<Option<ClarificationReceipt>, GuardianRejection>, StoreError> {
        Ok(Ok(self.store.clarify(clarification)?))
    }

    #[cfg(feature = "agent-guardian")]
    async fn guard_clarify(
        &mut self,
        clarification: Clarification,
    ) -> Result<Result<Option<ClarificationReceipt>, GuardianRejection>, StoreError> {
        let operation = GuardianOperation::clarify(clarification.clone());
        let authorization_operation = operation.clone();
        if let Some(rejection) = self.guard_model(operation)? {
            return Ok(Err(rejection));
        }
        self.authorize_guardian_operation(&authorization_operation)
            .await?;
        Ok(Ok(self.store.clarify(clarification)?))
    }

    #[cfg(not(feature = "agent-guardian"))]
    async fn guard_resolve_clarification(
        &mut self,
        resolution: ClarificationResolution,
    ) -> Result<Result<Option<ClarificationResolutionReceipt>, GuardianRejection>, StoreError> {
        Ok(Ok(self.store.resolve_clarification(resolution)?))
    }

    #[cfg(feature = "agent-guardian")]
    async fn guard_resolve_clarification(
        &mut self,
        resolution: ClarificationResolution,
    ) -> Result<Result<Option<ClarificationResolutionReceipt>, GuardianRejection>, StoreError> {
        let operation = GuardianOperation::resolve_clarification(resolution.clone());
        let authorization_operation = operation.clone();
        if let Some(rejection) = self.guard_model(operation)? {
            return Ok(Err(rejection));
        }
        self.authorize_guardian_operation(&authorization_operation)
            .await?;
        Ok(Ok(self.store.resolve_clarification(resolution)?))
    }

    #[cfg(not(feature = "agent-guardian"))]
    async fn guard_supersede(
        &mut self,
        supersession: Supersession,
    ) -> Result<Result<Option<SupersessionReceipt>, GuardianRejection>, StoreError> {
        Ok(Ok(self.store.supersede(supersession)?))
    }

    #[cfg(feature = "agent-guardian")]
    async fn guard_supersede(
        &mut self,
        supersession: Supersession,
    ) -> Result<Result<Option<SupersessionReceipt>, GuardianRejection>, StoreError> {
        let operation = GuardianOperation::supersede(supersession.clone());
        let authorization_operation = operation.clone();
        if let Some(rejection) = self.guard_model(operation)? {
            return Ok(Err(rejection));
        }
        self.authorize_guardian_operation(&authorization_operation)
            .await?;
        Ok(Ok(self.store.supersede(supersession)?))
    }

    #[cfg(not(feature = "agent-guardian"))]
    async fn guard_retire(
        &mut self,
        retirement: Retirement,
    ) -> Result<Result<Option<RetirementReceipt>, GuardianRejection>, StoreError> {
        Ok(Ok(self.store.retire(retirement)?))
    }

    #[cfg(not(feature = "agent-guardian"))]
    async fn guard_change_record(
        &mut self,
        change: RecordChange,
    ) -> Result<Result<Option<RecordChangeReceipt>, GuardianRejection>, StoreError> {
        Ok(Ok(self.store.change_record(change)?))
    }

    #[cfg(feature = "agent-guardian")]
    async fn guard_change_record(
        &mut self,
        change: RecordChange,
    ) -> Result<Result<Option<RecordChangeReceipt>, GuardianRejection>, StoreError> {
        let operation = GuardianOperation::change_record(change.clone());
        let authorization_operation = operation.clone();
        if let Some(rejection) = self.guard_model(operation)? {
            return Ok(Err(rejection));
        }
        self.authorize_guardian_operation(&authorization_operation)
            .await?;
        Ok(Ok(self.store.change_record(change)?))
    }

    #[cfg(feature = "agent-guardian")]
    async fn guard_retire(
        &mut self,
        retirement: Retirement,
    ) -> Result<Result<Option<RetirementReceipt>, GuardianRejection>, StoreError> {
        let operation = GuardianOperation::retire(retirement.clone());
        let authorization_operation = operation.clone();
        if let Some(rejection) = self.guard_model(operation)? {
            return Ok(Err(rejection));
        }
        self.authorize_guardian_operation(&authorization_operation)
            .await?;
        Ok(Ok(self.store.retire(retirement)?))
    }

    #[cfg(feature = "agent-guardian")]
    fn guard_model(
        &mut self,
        operation: GuardianOperation,
    ) -> Result<Option<GuardianRejection>, StoreError> {
        let Some(guardian) = &self.guardian else {
            if !self.guardian_required {
                return Ok(None);
            }
            let records = self.store.guardian_records_for_operation(&operation)?;
            let database_marker = self.store.database_marker();
            let verdict = nexus_schema::GuardianVerdict::reject(nexus_schema::GuardianReject {
                guardian_rejection_reason: GuardianRejectionReason::HarnessUnavailable,
                explanation: Explanation::new(
                    "guardian is required but no guardian agent is configured",
                ),
            });
            self.store.record_guardian_decision(
                crate::guardian_journal::GuardianDecision::record(
                    operation,
                    records.clone(),
                    verdict,
                    database_marker.clone(),
                ),
            )?;
            return Ok(Some(GuardianRejection {
                guardian_rejection_reason: GuardianRejectionReason::HarnessUnavailable,
                record_set: records,
                explanation: Explanation::new(
                    "guardian is required but no guardian agent is configured",
                ),
            }));
        };
        let records = self.store.guardian_records_for_operation(&operation)?;
        let decision = guardian.guard(&operation, records, self.store.database_marker());
        let rejection = decision.clone().into_guardian_rejection();
        self.store
            .record_guardian_decision(decision.journal_decision(operation))?;
        Ok(rejection)
    }

    #[cfg(not(feature = "agent-guardian"))]
    async fn guard_referent_registration(
        &mut self,
        registration: ReferentRegistration,
    ) -> Result<Result<ReferentRegistrationReceipt, ReferentGuardianRejection>, StoreError> {
        Ok(Ok(self.store.register_referent(registration)?))
    }

    #[cfg(feature = "agent-guardian")]
    async fn guard_referent_registration(
        &mut self,
        registration: ReferentRegistration,
    ) -> Result<Result<ReferentRegistrationReceipt, ReferentGuardianRejection>, StoreError> {
        if let Some(receipt) = self
            .store
            .settled_referent_registration_receipt(&registration)?
        {
            return Ok(Ok(receipt));
        }
        let Some(guardian) = &self.guardian else {
            if !self.guardian_required {
                return Ok(Ok(self.store.register_referent(registration)?));
            }
            let registered_referents = self.store.registered_referents()?;
            let database_marker = self.store.database_marker();
            let verdict = nexus_schema::ReferentGuardianVerdict::reject_referent(
                nexus_schema::ReferentReject {
                    referent_guardian_rejection_reason:
                        ReferentGuardianRejectionReason::HarnessUnavailable,
                    explanation: Explanation::new(
                        "guardian is required but no guardian agent is configured",
                    ),
                },
            );
            self.store.record_guardian_decision(
                crate::guardian_journal::GuardianDecision::referent(
                    registration,
                    registered_referents.clone(),
                    verdict,
                    database_marker.clone(),
                ),
            )?;
            return Ok(Err(ReferentGuardianRejection {
                referent_guardian_rejection_reason:
                    ReferentGuardianRejectionReason::HarnessUnavailable,
                registered_referents,
                explanation: Explanation::new(
                    "guardian is required but no guardian agent is configured",
                ),
            }));
        };
        let registered_referents = self.store.registered_referents()?;
        let decision = guardian.guard_referent(
            &registration,
            registered_referents,
            self.store.database_marker(),
        );
        let rejection = decision.clone().into_guardian_rejection();
        self.store
            .record_guardian_decision(decision.journal_decision(registration.clone()))?;
        match rejection {
            Some(rejection) => Ok(Err(rejection)),
            None => Ok(Ok(self.store.register_referent(registration)?)),
        }
    }

    async fn register_implied_referents(
        &mut self,
        entry: &Entry,
        justification: &Justification,
    ) -> Result<Result<(), ReferentGuardianRejection>, StoreError> {
        for referent in entry.referents.payload() {
            let registration = ReferentRegistration {
                referent: referent.clone(),
                aliases: Aliases::new(Referents::new(Vec::new())),
                justification: justification.clone(),
            };
            if let Err(rejection) = self.guard_referent_registration(registration).await? {
                return Ok(Err(rejection));
            }
        }
        Ok(Ok(()))
    }

    #[cfg(feature = "agent-guardian")]
    fn duplicate_rejection_if_needed(
        &self,
        entry: &Entry,
        rejection: GuardianRejection,
    ) -> Result<GuardianRejection, StoreError> {
        if rejection.guardian_rejection_reason
            == crate::schema::signal::GuardianRejectionReason::Duplicate
        {
            self.store
                .apply_duplicate_guardian_rejection(entry, rejection)
        } else {
            Ok(rejection)
        }
    }

    fn operation_failed(&self, message: impl Into<String>) -> NexusEffectResult {
        NexusEffectResult::operation_failed(ErrorReport::new(ErrorMessage::new(message.into())))
    }

    #[cfg(all(feature = "agent-guardian", feature = "criome-gate"))]
    async fn authorize_guardian_operation(
        &self,
        operation: &GuardianOperation,
    ) -> Result<(), StoreError> {
        let context = operation.authorization_context(self.operation_authorizer.process_key());
        match self
            .operation_authorizer
            .authorize(context, self.operation_authorization_mode)
            .await
            .map_err(|error| StoreError::CriomeAuthorization(error.to_string()))?
        {
            crate::criome_gate::SpiritOperationAuthorization::Allowed => Ok(()),
            crate::criome_gate::SpiritOperationAuthorization::Blocked(reason) => {
                Err(StoreError::CriomeAuthorization(reason))
            }
        }
    }

    #[cfg(all(feature = "agent-guardian", not(feature = "criome-gate")))]
    async fn authorize_guardian_operation(
        &self,
        _operation: &GuardianOperation,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    #[cfg(feature = "testing-trace")]
    fn trace_direct_sema_write(&self) {
        self.trace_log.record(TraceEvent::new(ObjectName::Sema(
            crate::schema::sema::SemaObjectName::WriteApplied,
        )));
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

    /// Run effect work inside the async Nexus loop.
    async fn apply_effect_operation(&mut self, command: NexusEffectCommand) -> NexusEffectResult {
        self.apply_effect(command).await
    }

    pub async fn execute_to_reply(
        &mut self,
        input: nexus_schema::nexus::Nexus<NexusWork>,
    ) -> nexus_schema::nexus::Nexus<NexusAction> {
        let origin_route = input.origin_route();
        let mut work = input.into_root();
        let mut budget = ContinuationLimit::default().budget();
        loop {
            let action = self.traced_step_decide(work);
            match action {
                NexusAction::ReplyToSignal(_) => return action.with_origin_route(origin_route),
                NexusAction::CommandSemaWrite(command) => {
                    let Err(exhausted) = budget.spend_next_step() else {
                        let output = self.apply_sema_write_operation(
                            command
                                .into_sema_write_input()
                                .with_origin_route(origin_route.into()),
                        );
                        work = NexusWork::sema_write_completed(output);
                        continue;
                    };
                    return NexusAction::reply_to_signal(self.budget_exhausted_reply(exhausted))
                        .with_origin_route(origin_route);
                }
                NexusAction::CommandSemaRead(command) => {
                    let Err(exhausted) = budget.spend_next_step() else {
                        let output = self.observe_sema_read_operation(
                            command.with_origin_route(origin_route.into()),
                        );
                        work = NexusWork::sema_read_completed(output);
                        continue;
                    };
                    return NexusAction::reply_to_signal(self.budget_exhausted_reply(exhausted))
                        .with_origin_route(origin_route);
                }
                NexusAction::CommandEffect(command) => {
                    let Err(exhausted) = budget.spend_next_step() else {
                        let output = self.apply_effect_operation(command).await;
                        work = NexusWork::effect_completed(output);
                        continue;
                    };
                    return NexusAction::reply_to_signal(self.budget_exhausted_reply(exhausted))
                        .with_origin_route(origin_route);
                }
                NexusAction::Continue(next) => {
                    let Err(exhausted) = budget.spend_next_step() else {
                        work = next;
                        continue;
                    };
                    return NexusAction::reply_to_signal(self.budget_exhausted_reply(exhausted))
                        .with_origin_route(origin_route);
                }
            }
        }
    }

    fn budget_exhausted_reply(&self, exhausted: ContinuationExhausted) -> Output {
        Output::error(ErrorReport::new(ErrorMessage::new(format!(
            "nexus continuation budget exhausted after {} steps (limit {})",
            exhausted.completed_step_count(),
            exhausted.limit().count()
        ))))
    }
}

/// Generated `NexusEngine` owns lifecycle and one-step decision dispatch.
/// Spirit drives the recursive runner loop in `Nexus::execute_to_reply` because
/// the no-alias schema shape no longer emits the old multi-hook runner trait.
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
        self.apply_effect_operation(input).await
    }

    fn budget_exhausted_reply(&self, exhausted: ContinuationExhausted) -> Output {
        Nexus::budget_exhausted_reply(self, exhausted)
    }

    fn decide(
        &mut self,
        input: nexus_schema::nexus::Nexus<nexus_schema::nexus::Work>,
    ) -> nexus_schema::nexus::Nexus<nexus_schema::nexus::Action> {
        let origin_route = input.origin_route();
        self.traced_step_decide(input.into_root())
            .with_origin_route(origin_route)
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
    /// (Stashed) feedback becomes `Output::RecordsStashed`, carrying
    /// both the stash handle and the observed records.
    /// State classification also lives here as a schema-declared
    /// `CommandEffect(ClassifyState)` followed by
    /// `EffectCompleted(StateClassified)` and the ordinary SEMA
    /// `Record` write.
    fn traced_step_decide(&mut self, work: NexusWork) -> NexusAction {
        self.trace_nexus_entered();
        let action = self.step_decide(work);
        self.trace_nexus_decided();
        action
    }

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
            Input::State(statement) => NexusAction::command_effect(
                NexusEffectCommand::classify_state(statement.into_payload()),
            ),
            Input::Record(record) => NexusAction::command_effect(
                NexusEffectCommand::record_with_implied_referents(record.into_payload()),
            ),
            Input::Propose(propose) => NexusAction::command_effect(
                NexusEffectCommand::propose_with_implied_referents(propose.into_payload()),
            ),
            Input::Clarify(clarify) => {
                NexusAction::command_effect(NexusEffectCommand::clarify(clarify.into_payload()))
            }
            Input::ResolveClarification(resolution) => NexusAction::command_effect(
                NexusEffectCommand::resolve_clarification(resolution.into_payload()),
            ),
            Input::Supersede(supersede) => NexusAction::command_effect(
                NexusEffectCommand::supersede_with_implied_referents(supersede.into_payload()),
            ),
            Input::Retire(retire) => {
                NexusAction::command_effect(NexusEffectCommand::retire(retire.into_payload()))
            }
            Input::Observe(observe) => {
                NexusAction::command_sema_read(SemaReadInput::observe(observe.into_payload()))
            }
            Input::PublicIntent(public_intent) => NexusAction::command_sema_read(
                SemaReadInput::public_intent(public_intent.into_payload()),
            ),
            Input::PublicTextSearch(search) => NexusAction::command_sema_read(
                SemaReadInput::public_text_search(search.into_payload()),
            ),
            Input::PublicRecords(selection) => NexusAction::command_sema_read(
                SemaReadInput::observe(selection.into_payload().into_public_query()),
            ),
            Input::PrivateRecords(selection) => NexusAction::command_sema_read(
                SemaReadInput::observe(selection.into_payload().into_private_query()),
            ),
            Input::Lookup(lookup) => {
                NexusAction::command_sema_read(SemaReadInput::lookup(lookup.into_payload()))
            }
            Input::Count(count) => {
                NexusAction::command_sema_read(SemaReadInput::count(count.into_payload()))
            }
            Input::ChangeCertainty(change) => NexusAction::command_sema_write(
                CommandSemaWrite::change_certainty(change.into_payload()),
            ),
            Input::BumpImportance(change) => NexusAction::command_sema_write(
                CommandSemaWrite::bump_importance(change.into_payload()),
            ),
            Input::ChangeRecord(change) => NexusAction::command_effect(
                NexusEffectCommand::change_record_with_implied_referents(change.into_payload()),
            ),
            Input::RegisterReferent(register) => NexusAction::command_effect(
                NexusEffectCommand::guard_referent_registration(register.into_payload()),
            ),
            Input::LookupStash(handle) => match self.stash_table.lookup(handle.payload()) {
                Some((records, _database_marker)) => NexusAction::reply_to_signal(
                    Output::records_observed(crate::schema::signal::ObservedRecords::new(
                        crate::schema::signal::RecordSet::new(records.into_payload()),
                    )),
                ),
                None => NexusAction::reply_to_signal(Output::rejected(SignalRejection::new(
                    ValidationError::StashHandleNotFound,
                ))),
            },
            Input::Tap(filter) => NexusAction::command_effect(
                NexusEffectCommand::open_observer_tap(filter.into_payload()),
            ),
            Input::Untap(token) => NexusAction::command_effect(
                NexusEffectCommand::close_observer_tap(token.into_payload()),
            ),
            Input::SubscribeIntent(query) => NexusAction::command_effect(
                NexusEffectCommand::open_intent_subscription(query.into_payload()),
            ),
            Input::Version => NexusAction::reply_to_signal(Output::version_reported(
                VersionReport::new(VersionText::new(env!("CARGO_PKG_VERSION"))),
            )),
            Input::Marker => {
                NexusAction::reply_to_signal(Output::marker_reported(self.database_marker()))
            }
            // The authorized-apply ingress stays parked until the §4
            // propagation slice reactivates it in batch form
            // (acceptance-by-verification at the receiving criome — it is
            // NOT intake-gated: the carried authorization already happened).
            // Until then Spirit never accepts a foreign authorized-record
            // apply on the working socket; the contract retains the variant,
            // so the daemon answers it fail-closed — no criome round-trip,
            // no store write.
            Input::ApplyAuthorizedRecord(_) => NexusAction::reply_to_signal(Output::apply_refused(
                ApplyRefusal::new(ApplyRefusalReason::AuthorizationUnavailable),
            )),
        }
    }

    fn decide_sema_write_completion(&self, output: SemaWriteOutput) -> NexusAction {
        match output {
            SemaWriteOutput::Recorded(receipt) => {
                NexusAction::reply_to_signal(Output::record_accepted(receipt.record_identifier))
            }
            SemaWriteOutput::CertaintyChanged(receipt) => {
                NexusAction::reply_to_signal(Output::certainty_changed(receipt))
            }
            SemaWriteOutput::ImportanceBumped(receipt) => {
                NexusAction::reply_to_signal(Output::importance_bumped(receipt))
            }
            SemaWriteOutput::RecordChanged(receipt) => {
                NexusAction::reply_to_signal(Output::record_changed(receipt))
            }
            SemaWriteOutput::ReferentRegistered(receipt) => {
                NexusAction::reply_to_signal(Output::referent_registered(receipt))
            }
            SemaWriteOutput::Missed(report) => NexusAction::reply_to_signal(Output::error(report)),
        }
    }

    fn decide_sema_read_completion(&self, output: SemaReadOutput) -> NexusAction {
        match output {
            SemaReadOutput::Observed(observed) => {
                // Observe recurses through Stash so the reply carries
                // both a recovery handle and the record set.
                let records = Records::new(observed.into_payload().into_payload());
                NexusAction::command_effect(NexusEffectCommand::stash(StashRequest {
                    records,
                    database_marker: self.database_marker(),
                }))
            }
            SemaReadOutput::PublicIntentResults(observed) => {
                NexusAction::reply_to_signal(Output::records_observed(observed))
            }
            SemaReadOutput::PublicTextSearchResults(observed) => {
                NexusAction::reply_to_signal(Output::records_observed(observed))
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
            NexusEffectResult::StateClassified(entry) => NexusAction::command_effect(
                NexusEffectCommand::record_with_implied_referents(entry),
            ),
            NexusEffectResult::RecordReferentsSettled(record) => {
                #[cfg(feature = "agent-guardian")]
                {
                    NexusAction::command_effect(NexusEffectCommand::guard_record(record))
                }
                #[cfg(not(feature = "agent-guardian"))]
                {
                    NexusAction::command_sema_write(CommandSemaWrite::record(record.entry))
                }
            }
            NexusEffectResult::ProposeReferentsSettled(propose) => {
                NexusAction::command_effect(NexusEffectCommand::propose(propose))
            }
            NexusEffectResult::SupersedeReferentsSettled(supersede) => {
                NexusAction::command_effect(NexusEffectCommand::supersede(supersede))
            }
            NexusEffectResult::ChangeRecordReferentsSettled(change) => {
                #[cfg(feature = "agent-guardian")]
                {
                    NexusAction::command_effect(NexusEffectCommand::guard_change_record(change))
                }
                #[cfg(not(feature = "agent-guardian"))]
                {
                    NexusAction::command_sema_write(CommandSemaWrite::change_record(change))
                }
            }
            NexusEffectResult::Recorded(receipt) => {
                NexusAction::reply_to_signal(Output::record_accepted(receipt.record_identifier))
            }
            NexusEffectResult::Proposed(receipt) => {
                NexusAction::reply_to_signal(Output::proposed(receipt.record_identifier))
            }
            NexusEffectResult::Clarified(receipt) => {
                NexusAction::reply_to_signal(Output::clarified(receipt))
            }
            NexusEffectResult::ClarificationResolved(receipt) => {
                NexusAction::reply_to_signal(Output::clarification_resolved(receipt))
            }
            NexusEffectResult::Superseded(receipt) => {
                NexusAction::reply_to_signal(Output::superseded(receipt))
            }
            NexusEffectResult::Retired(receipt) => {
                NexusAction::reply_to_signal(Output::retired(receipt))
            }
            NexusEffectResult::RecordChanged(receipt) => {
                NexusAction::reply_to_signal(Output::record_changed(receipt))
            }
            NexusEffectResult::OperationFailed(report) => {
                NexusAction::reply_to_signal(Output::error(report))
            }
            NexusEffectResult::GuardianRejected(rejection) => {
                NexusAction::reply_to_signal(Output::guardian_rejected(rejection))
            }
            NexusEffectResult::ReferentRegistered(receipt) => {
                NexusAction::reply_to_signal(Output::referent_registered(receipt))
            }
            NexusEffectResult::ReferentGuardianRejected(rejection) => {
                NexusAction::reply_to_signal(Output::referent_guardian_rejected(rejection))
            }
            NexusEffectResult::Stashed(stashed) => {
                let StashResult {
                    stash_handle,
                    record_count,
                    database_marker: _database_marker,
                    records,
                } = stashed;
                NexusAction::reply_to_signal(Output::records_stashed(StashedObservation {
                    stash_handle,
                    record_count,
                    observed_records: ObservedRecords::new(RecordSet::new(records.into_payload())),
                }))
            }
            NexusEffectResult::IntentSubscriptionOpened(subscription) => {
                NexusAction::reply_to_signal(Output::subscription_started(subscription))
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
