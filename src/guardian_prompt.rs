use signal_agent::{
    ChatMessage, ChatTranscript, CompletionText, MaximumOutputTokens, ModelName, OutputMode,
    Prompt, PromptOptions, ProviderName, SystemText, TemperatureMilli,
};

use crate::{
    guardian_journal::GuardianOperation,
    schema::{
        nexus::{GuardianVerdict, ReferentGuardianVerdict, Reject, RejectReferent},
        signal::{
            Explanation, GuardianRejectionReason, RecordSet, ReferentGuardianRejectionReason,
            ReferentRegistration, RegisteredReferents,
        },
    },
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct GuardianPromptBuilder<'configuration> {
    provider_name: Option<&'configuration str>,
    model_name: Option<&'configuration str>,
    maximum_output_tokens: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuardianRetry {
    completion: CompletionText,
    error: String,
}

struct GuardianOperationPrompt<'operation> {
    operation: &'operation GuardianOperation,
}

impl<'configuration> GuardianPromptBuilder<'configuration> {
    pub(crate) fn new(
        provider_name: Option<&'configuration str>,
        model_name: Option<&'configuration str>,
        maximum_output_tokens: Option<u64>,
    ) -> Self {
        Self {
            provider_name,
            model_name,
            maximum_output_tokens,
        }
    }

    pub(crate) fn guardian_prompt(
        &self,
        operation: &GuardianOperation,
        records: &RecordSet,
        retry: Option<&GuardianRetry>,
    ) -> Prompt {
        let accept = GuardianVerdict::Accept.to_nota();
        let reject = GuardianVerdict::reject(Reject {
            guardian_rejection_reason: GuardianRejectionReason::Contradiction,
            explanation: Explanation::new("explain the conflict"),
        })
        .to_nota();
        let retry_text = retry
            .map(|retry| retry.user_text("GuardianVerdict"))
            .unwrap_or_default();
        Prompt {
            system: Some(SystemText::new(format!(
                "You are Spirit's guardian. Judge whether one write operation may change the live intent store. The model checks every semantic admission issue: duplicates, contradictions, compound arrows, non-intent, unclear privacy, unclear domain, clarification damage, supersession damage, and retrieval insufficiency. Code only validates structure and applies the typed verdict. Decide directly. Reply with exactly one NOTA value of the GuardianVerdict type in the final assistant content and no surrounding prose. Accept exactly as {accept}. Reject with the nested form exactly like {reject}; never write a flat rejection like (Reject Reason [explanation]). The candidate operation may contain an Entry plus a Justification; the Justification is source evidence for admission, not a second intent arrow. A brief Justification, or one that repeats the Entry, is enough when the Entry itself is clear; do not reject merely because the Justification is not an external citation. Closed rejection reasons: Duplicate, Contradiction, Compound, NonIntent, UnclearPrivacy, UnclearDomain, ClarifyTramples, ClarifyLosesMeaning, SupersedeTargetMissing, RetrievalInsufficient, HarnessUnavailable, HarnessMalformed, HarnessTimedOut. Duplicate means the candidate already says the same forward arrow as a live record. Contradiction means it conflicts with a live arrow. Compound means the Entry itself bundles multiple separable arrows. NonIntent means the Entry is task chatter, status, or transient reaction rather than durable intent. UnclearPrivacy means the privacy level is unsafe or underspecified. UnclearDomain means the domain classification is missing or wrong enough to block admission. ClarifyTramples means a clarification overwrites the prior arrow instead of making it clearer. ClarifyLosesMeaning means a clarification drops material meaning. SupersedeTargetMissing means the replacement cannot be judged against the claimed target. RetrievalInsufficient means the supplied relevant-existing-record bundle is too thin to judge duplicates, contradictions, or target-sensitive mutations; it does not mean the Justification lacks external proof."
            ))),
            transcript: ChatTranscript::new(vec![ChatMessage::user(format!(
                "Operation: {}\n\nCandidate:\n{}\n\nRelevant existing records:\n{}{}",
                operation.name(),
                GuardianOperationPrompt::new(operation).to_text(),
                records.to_nota(),
                retry_text
            ))]),
            options: self.prompt_options(),
        }
    }

    pub(crate) fn referent_prompt(
        &self,
        registration: &ReferentRegistration,
        registered_referents: &RegisteredReferents,
        retry: Option<&GuardianRetry>,
    ) -> Prompt {
        let accept = ReferentGuardianVerdict::Accept.to_nota();
        let reject = ReferentGuardianVerdict::reject_referent(RejectReferent {
            referent_guardian_rejection_reason: ReferentGuardianRejectionReason::Duplicate,
            explanation: Explanation::new("explain the referent problem"),
        })
        .to_nota();
        let retry_text = retry
            .map(|retry| retry.user_text("ReferentGuardianVerdict"))
            .unwrap_or_default();
        Prompt {
            system: Some(SystemText::new(format!(
                "You are Spirit's referent guardian. Judge whether one referent registration may change the referent registry. The model checks every semantic admission issue: duplicates, ambiguity, vague names, alias collisions, non-referents, and unclear justification. Code only validates structure and applies the typed verdict. Decide directly. Reply with exactly one NOTA value of the ReferentGuardianVerdict type in the final assistant content and no surrounding prose. Accept exactly as {accept}. Reject with the nested form exactly like {reject}; never write a flat rejection like (RejectReferent Reason [explanation]). Closed rejection reasons: Duplicate, Ambiguous, TooVague, AliasCollision, NonReferent, UnclearJustification, HarnessUnavailable, HarnessMalformed, HarnessTimedOut. Duplicate means the requested referent or alias already names an existing referent. Ambiguous means the name could naturally point to multiple particulars. TooVague means the name is not specific enough to identify one concrete thing. AliasCollision means an alias collides with another registered referent. NonReferent means the candidate is not a concrete named thing. UnclearJustification means the psyche statement does not justify registering this referent."
            ))),
            transcript: ChatTranscript::new(vec![ChatMessage::user(format!(
                "Operation: RegisterReferent\n\nCandidate registration:\n{}\n\nRegistered referents:\n{}{}",
                registration.to_nota(),
                registered_referents.to_nota(),
                retry_text
            ))]),
            options: self.prompt_options(),
        }
    }

    fn prompt_options(&self) -> PromptOptions {
        PromptOptions {
            model: self
                .model_name
                .map(|model| ModelName::new(model.to_owned())),
            provider: self
                .provider_name
                .map(|provider| ProviderName::new(provider.to_owned())),
            temperature_milli: Some(TemperatureMilli::new(0)),
            maximum_output_tokens: self.maximum_output_tokens.map(MaximumOutputTokens::new),
            output_mode: OutputMode::Nota,
        }
    }
}

impl GuardianRetry {
    pub(crate) fn new(completion: CompletionText, error: String) -> Self {
        Self { completion, error }
    }

    fn user_text(&self, verdict_type: &str) -> String {
        format!(
            "\n\nPrevious response was not a {verdict_type}:\n{}\n\nParse error:\n{}\n\nReturn only the corrected {verdict_type} NOTA value.",
            self.completion.payload(),
            self.error
        )
    }
}

impl<'operation> GuardianOperationPrompt<'operation> {
    fn new(operation: &'operation GuardianOperation) -> Self {
        Self { operation }
    }

    fn to_text(&self) -> String {
        match self.operation {
            GuardianOperation::Record(request) => {
                format!("Record request:\n{}", request.to_nota())
            }
            GuardianOperation::Propose(proposal) => {
                format!("Propose request:\n{}", proposal.to_nota())
            }
            GuardianOperation::Clarify(clarification) => {
                format!("Clarify request:\n{}", clarification.to_nota())
            }
            GuardianOperation::Supersede(supersession) => {
                format!("Supersede request:\n{}", supersession.to_nota())
            }
            GuardianOperation::Retire(retirement) => {
                format!("Retire request:\n{}", retirement.to_nota())
            }
            GuardianOperation::Remove(removal) => {
                format!("Remove request:\n{}", removal.to_nota())
            }
            GuardianOperation::ChangeRecord(change) => {
                format!("ChangeRecord request:\n{}", change.to_nota())
            }
            GuardianOperation::CollectRemovalCandidates(collection) => {
                format!(
                    "CollectRemovalCandidates request:\n{}",
                    collection.to_nota()
                )
            }
        }
    }
}
