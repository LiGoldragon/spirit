//! The guardian prompt: how Spirit asks a specialized, clean-context LLM judge
//! to render one binary admission verdict over a candidate intent write.
//!
//! The guardian is a court of law. The submitting agent is the advocate; the
//! typed `Justification` is its argued case; the verbatim `Testimony` is the
//! evidence; the claimed `Certainty` is the burden of proof that testimony must
//! clear; the guardian is the judge. The verdict is strictly binary — admit or
//! remand — and the guardian never edits a magnitude itself (record `woku`).
//!
//! Two properties are load-bearing and are enforced structurally rather than by
//! hope: (1) the closed rejection-reason set and the exact `(Accept)` /
//! `(Reject (<Reason> [explanation]))` verdict grammar are RENDERED FROM THE
//! ENUM via `NotaEncode`, so the prompt can never drift from the wire type the
//! daemon parses (record `4jgt`); (2) the certainty gate reads modality off the
//! verbatim quote, never the agent's prose (record `so3b`). The judge is
//! over-trained on contrastive accept/reject pairs (record `88mw`) and asks the
//! provider for thinking-enabled, high-effort reasoning.

use signal_agent::{
    ChatMessage, ChatTranscript, CompletionText, MaximumOutputTokens, ModelName, OutputMode,
    Prompt, PromptOptions, ProviderName, ReasoningEffort, SystemText, TemperatureMilli,
    ThinkingMode,
};

use nota::NotaEncode;

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

/// The model-emittable rejection reasons (the daemon-only `Harness*` variants
/// are excluded — the model never emits them). The order is presentation
/// grouping, not gate order; the gate sequence lives in `GUARDIAN_CHECKLIST`. The
/// guardian is shown exactly this set; the daemon-only `Harness*` reasons are
/// excluded because the model never emits them — they are set on transport
/// failure. `GuardianRejectionReason::admission_gloss` is exhaustive, so adding
/// a variant to the enum forces a decision here.
const MODEL_REASONS: [GuardianRejectionReason; 17] = [
    GuardianRejectionReason::MissingTestimony,
    GuardianRejectionReason::TestimonyFabricated,
    GuardianRejectionReason::InsufficientWarrant,
    GuardianRejectionReason::Overstated,
    GuardianRejectionReason::ImportanceUnsupported,
    GuardianRejectionReason::NonIntent,
    GuardianRejectionReason::NegativeGuideline,
    GuardianRejectionReason::Matter,
    GuardianRejectionReason::Compound,
    GuardianRejectionReason::UnclearDomain,
    GuardianRejectionReason::UnclearPrivacy,
    GuardianRejectionReason::Duplicate,
    GuardianRejectionReason::Contradiction,
    GuardianRejectionReason::ClarifyTramples,
    GuardianRejectionReason::ClarifyLosesMeaning,
    GuardianRejectionReason::SupersedeTargetMissing,
    GuardianRejectionReason::RetrievalInsufficient,
];

trait GuardianRejectionReasonPromptExt {
    fn admission_gloss(&self) -> Option<&'static str>;
}

impl GuardianRejectionReasonPromptExt for GuardianRejectionReason {
    /// The one-line gloss shown to the guardian for each model-emittable reason.
    /// `Harness*` reasons return `None`: the model never names them; the daemon
    /// sets them when the agent call itself fails. The match is exhaustive on
    /// purpose — a new reason variant will not compile until it is glossed.
    fn admission_gloss(&self) -> Option<&'static str> {
        match self {
            Self::MissingTestimony => Some(
                "no verbatim quote at all, or a bare affirmation (yes / do it) with no antecedent. \
                 An evidentiary production failure — NOT RetrievalInsufficient, NOT NonIntent.",
            ),
            Self::TestimonyFabricated => Some(
                "a quote that reads like agent prose rather than how a human actually talks, or one \
                 suspiciously perfect for the claim. Heuristic only.",
            ),
            Self::InsufficientWarrant => Some(
                "the reasoning and quotes do not license THIS submission: on-point words that do \
                 not argue this point, or a destructive op (Supersede / Retire / meaning-changing \
                 ChangeRecord / certainty downgrade) carrying no verbatim psyche authorization, \
                 or a fresh Record whose evidence warrants editing an existing target or resolving \
                 a live conflict through maintenance instead.",
            ),
            Self::Overstated => Some(
                "the claimed Certainty outruns the quote's modal strength (e.g. High claimed on \
                 could / maybe / I think). Read modality off the QUOTE, never the agent's framing.",
            ),
            Self::ImportanceUnsupported => Some(
                "the Importance rung is not justified from its OWN evidence (direct psyche \
                 declaration, recurrence, architectural centrality, blast radius, \
                 keeps-coming-up, or blocks-other-work). Confident tone is not evidence of \
                 importance.",
            ),
            Self::NonIntent => Some(
                "task chatter, a status update, or a transient reaction — not durable intent that \
                 still guides once the current task is erased.",
            ),
            Self::NegativeGuideline => Some(
                "the candidate's operative guidance is framed primarily as an exclusion, \
                 prohibition, forbidden wording, or definition-by-negation. Remand: state the \
                 affirmative shape to follow.",
            ),
            Self::Matter => Some(
                "durable, but not the psyche's intent. Intent is a durable UNIVERSAL RULE the \
                 psyche wants — a standing direction about the work or the world, and it is rare. \
                 Matter is concrete content instead: (a) scoped to a single mechanism, component, \
                 or one-off architectural decision — code, an architecture, a specification, a \
                 mechanism, a manual entry, or what Spirit / the guardian / the system / a process \
                 IS; or (b) Spirit-usage / agent-training material — any rule about how to use, \
                 operate, or interpret Spirit itself. STRIP TEST: an action performed on or with \
                 Spirit is matter; a general work-or-world behavior Spirit merely records is \
                 intent. Matter belongs written in the repository — a repo file, ARCHITECTURE doc, \
                 skill, or bead — not the intent database. Remand: name its proper home and record \
                 it there. When a record mixes a thin directive with such matter, treat the whole \
                 thing as Matter — the directive can be re-captured cleanly on its own later.",
            ),
            Self::Compound => {
                Some("the Entry bundles several separable arrows that belong in distinct records.")
            }
            Self::UnclearDomain => Some(
                "the domain is missing or wrong. A named instance (spirit, rkyv, DeepSeek) is a \
                 referent, not a domain; domains are universal subjects.",
            ),
            Self::UnclearPrivacy => Some(
                "the privacy level is unsafe or underspecified — e.g. personal-affairs substance \
                 recorded at Zero (public).",
            ),
            Self::Duplicate => Some(
                "a live record already states the same forward arrow. Remand: the daemon bumps the \
                 canonical record's importance.",
            ),
            Self::Contradiction => Some(
                "the candidate negates a live psyche arrow without a verbatim psyche quote \
                 authorizing the reversal. Remand: provide authorizing testimony, then file the \
                 coherent maintenance operation instead of a conflicting sibling Record.",
            ),
            Self::ClarifyTramples => Some(
                "a Clarify or ChangeRecord redirects, inverts, or hardens the original arrow \
                 instead of sharpening it — that is a Supersede, not a Clarify.",
            ),
            Self::ClarifyLosesMeaning => Some(
                "a Clarify, ChangeRecord, or collapsed multi-target Supersede silently drops \
                 material meaning the original record(s) carried.",
            ),
            Self::SupersedeTargetMissing => Some(
                "a named target record is absent from the supplied bundle, so the operation cannot \
                 be judged against what it claims to replace.",
            ),
            Self::RetrievalInsufficient => Some(
                "the supplied live-record bundle is too thin to judge duplicate / contradiction / \
                 target. An EMPTY bundle for a genuinely novel Record is fine — that is not \
                 insufficiency.",
            ),
            Self::HarnessUnavailable | Self::HarnessMalformed | Self::HarnessTimedOut => None,
        }
    }
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
        let retry_text = retry
            .map(|retry| retry.user_text("GuardianVerdict"))
            .unwrap_or_default();
        Prompt::new(
            Some(SystemText::new(Self::intent_guardian_system_prompt())),
            ChatTranscript::new(vec![ChatMessage::user(format!(
                "Operation: {operation}\n\nCandidate under judgement:\n{candidate}\n\nRelevant existing records (the live bundle — judge duplicates, contradictions, and named targets only against what appears here):\n{bundle}{retry_text}",
                operation = operation.name(),
                candidate = GuardianOperationPrompt::new(operation).to_text(),
                bundle = records.to_nota(),
            ))]),
            self.prompt_options(),
        )
    }

    pub(crate) fn referent_prompt(
        &self,
        registration: &ReferentRegistration,
        registered_referents: &RegisteredReferents,
        retry: Option<&GuardianRetry>,
    ) -> Prompt {
        let retry_text = retry
            .map(|retry| retry.user_text("ReferentGuardianVerdict"))
            .unwrap_or_default();
        Prompt::new(
            Some(SystemText::new(Self::referent_guardian_system_prompt())),
            ChatTranscript::new(vec![ChatMessage::user(format!(
                "Operation: RegisterReferent\n\nCandidate registration:\n{registration}\n\nAlready-registered referents:\n{registered}{retry_text}",
                registration = registration.to_nota(),
                registered = registered_referents.to_nota(),
            ))]),
            self.prompt_options(),
        )
    }

    /// The full intent-guardian system prompt: role, the record and justification
    /// shapes, the burden-of-proof ladder, the directed 11-gate checklist, the
    /// closed reason set and verdict grammar rendered from the enum, the NOTA
    /// output discipline, and the over-trained few-shot.
    fn intent_guardian_system_prompt() -> String {
        format!(
            "{role}\n\n{record}\n\n{justification}\n\n{ladder}\n\n{checklist}\n\n{reasons}\n\n{nota}\n\n{few_shot}",
            role = GUARDIAN_ROLE,
            record = GUARDIAN_RECORD_SHAPE,
            justification = GUARDIAN_JUSTIFICATION_SHAPE,
            ladder = GUARDIAN_BURDEN_LADDER,
            checklist = GUARDIAN_CHECKLIST,
            reasons = Self::reason_catalogue(),
            nota = Self::nota_verdict_discipline(),
            few_shot = GUARDIAN_FEW_SHOT,
        )
    }

    fn referent_guardian_system_prompt() -> String {
        let accept = ReferentGuardianVerdict::Accept.to_nota();
        let reject = ReferentGuardianVerdict::reject_referent(RejectReferent {
            referent_guardian_rejection_reason: ReferentGuardianRejectionReason::NonReferent,
            explanation: Explanation::new("a verb or concept is not a nameable particular"),
        })
        .to_nota();
        REFERENT_PROMPT_TEMPLATE
            .replace("{accept}", &accept)
            .replace("{reject}", &reject)
    }

    /// The closed rejection-reason catalogue, rendered FROM the enum so the
    /// prompt cannot drift from the wire type. Each line is the exact NOTA atom
    /// the daemon will parse, plus its gloss.
    fn reason_catalogue() -> String {
        let lines = MODEL_REASONS
            .iter()
            .filter_map(|reason| {
                reason
                    .admission_gloss()
                    .map(|gloss| format!("- {atom} — {gloss}", atom = reason.to_nota()))
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "CLOSED REJECTION REASONS. On a reject you name EXACTLY ONE of these atoms and no \
             other. They are the only legal reason atoms; never invent one. Never name a Harness \
             reason — those are the daemon's, not yours.\n{lines}"
        )
    }

    /// The NOTA output discipline plus the exact verdict grammar, both anchored
    /// to values rendered from the actual `GuardianVerdict` type.
    fn nota_verdict_discipline() -> String {
        let accept = GuardianVerdict::Accept.to_nota();
        let reject = GuardianVerdict::reject(Reject {
            guardian_rejection_reason: GuardianRejectionReason::Overstated,
            explanation: Explanation::new(
                "could and maybe are hedged and cannot clear High; the honest rung is Low",
            ),
        })
        .to_nota();
        format!(
            "NOTA OUTPUT — the one hard formatting law. Your entire reply is ONE NOTA value of the \
             GuardianVerdict type and NOTHING else: no prose before or after, no markdown fences, \
             no explanation outside the brackets, and NEVER a quotation mark (NOTA has no quoted \
             strings — free text goes in [square brackets]).\n\
             NOTA is type-first and positional: a record is (Head field field ...); an enum value \
             is its bare PascalCase head; free text is [bracketed]; there are no key:value pairs.\n\
             - To ADMIT, reply exactly: {accept}\n\
             - To REJECT, reply with the DOUBLE-NESTED form exactly like: {reject}\n\
             That is (Reject ( <ReasonAtom> [one short explanation sentence] )). The inner \
             parentheses are mandatory. A flat (Reject Overstated [..]) is MALFORMED. A bare atom, \
             a missing explanation bracket, an out-of-set reason, or any second value is MALFORMED \
             and will be rejected and retried. The explanation is one plain sentence in [brackets], \
             always present, never empty, no quotation marks inside."
        )
    }

    fn prompt_options(&self) -> PromptOptions {
        PromptOptions::new(
            self.model_name
                .map(|model| ModelName::new(model.to_owned())),
            self.provider_name
                .map(|provider| ProviderName::new(provider.to_owned())),
            // Temperature 0: the judge internalises the skeptic, hunting the
            // competing reading of a quote and the hedge that caps the claim.
            Some(TemperatureMilli::new(0)),
            self.maximum_output_tokens.map(MaximumOutputTokens::new),
            OutputMode::Nota,
            // A deliberate judge: thinking enabled at high effort. Higher
            // reasoning effort also improves exact-format adherence.
            Some(ReasoningEffort::High),
            Some(ThinkingMode::Enabled),
        )
    }
}

impl GuardianRetry {
    pub(crate) fn new(completion: CompletionText, error: String) -> Self {
        Self { completion, error }
    }

    fn user_text(&self, verdict_type: &str) -> String {
        format!(
            "\n\nYOUR PREVIOUS REPLY WAS NOT A VALID {verdict_type} and was discarded:\n{previous}\n\nThe parser said:\n{error}\n\nReply again with ONLY one valid {verdict_type} NOTA value — (Accept) or the double-nested (Reject ( <Reason> [explanation] )) — and nothing else.",
            previous = self.completion.payload(),
            error = self.error,
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
                format!(
                    "Record request (create a new durable intent):\n{}",
                    request.to_nota()
                )
            }
            GuardianOperation::Propose(proposal) => {
                format!(
                    "Propose request (create a new durable intent):\n{}",
                    proposal.to_nota()
                )
            }
            GuardianOperation::Clarify(clarification) => format!(
                "Clarify request (sharpen an existing record without changing its arrow):\n{}",
                clarification.to_nota()
            ),
            GuardianOperation::ResolveClarification(resolution) => format!(
                "ResolveClarification request (fold a standalone clarification record into its \
                 target record edits, then remove the standalone clarification):\n{}",
                resolution.to_nota()
            ),
            GuardianOperation::Supersede(supersession) => format!(
                "Supersede request (atomically retire the target set and install the replacement set; \
                 the replacements TOGETHER must preserve the retired kernel with no meaning lost):\n{}",
                supersession.to_nota()
            ),
            GuardianOperation::Retire(retirement) => format!(
                "Retire request (remove an intent the psyche no longer wants; needs a verbatim \
                 psyche authorization):\n{}",
                retirement.to_nota()
            ),
            GuardianOperation::ChangeRecord(change) => format!(
                "ChangeRecord request (edit a record in place under the same identifier; a \
                 correction, not a redirect):\n{}",
                change.to_nota()
            ),
        }
    }
}

const GUARDIAN_ROLE: &str = include_str!("guardian-prompts/role.md");
const REFERENT_PROMPT_TEMPLATE: &str = include_str!("guardian-prompts/referent.md");

const GUARDIAN_RECORD_SHAPE: &str = include_str!("guardian-prompts/record-shape.md");

const GUARDIAN_JUSTIFICATION_SHAPE: &str = include_str!("guardian-prompts/justification-shape.md");

const GUARDIAN_BURDEN_LADDER: &str = include_str!("guardian-prompts/burden-ladder.md");

const GUARDIAN_CHECKLIST: &str = include_str!("guardian-prompts/checklist.md");

/// The over-trained few-shot: contrastive accept/reject pairs, each grounded in
/// real software/schema/NOTA intent, each ending in the exact NOTA verdict. The
/// load-bearing pairs differ only in the thing under test (same quote at two
/// certainties; orthogonal axes; destructive authorization; multi-replacement
/// preservation; sharpen-vs-trample). Over-trained now; trimmed later only by
/// measured ablation that regresses neither verdict nor reason match.
const GUARDIAN_FEW_SHOT: &str = include_str!("guardian-prompts/few-shot.md");

#[cfg(test)]
mod tests {
    use super::*;
    use nota::NotaSource;

    #[test]
    fn accept_renders_bare_and_round_trips() {
        let rendered = GuardianVerdict::Accept.to_nota();
        assert_eq!(
            rendered, "Accept",
            "Accept must render as a bare atom, matching the few-shot"
        );
        let parsed = NotaSource::new(&rendered)
            .parse::<GuardianVerdict>()
            .expect("rendered Accept must parse back");
        assert!(matches!(parsed, GuardianVerdict::Accept));
    }

    #[test]
    fn reject_renders_double_nested_and_round_trips() {
        let rendered = GuardianVerdict::reject(Reject {
            guardian_rejection_reason: GuardianRejectionReason::Overstated,
            explanation: Explanation::new("could is hedged"),
        })
        .to_nota();
        assert!(
            rendered.starts_with("(Reject (Overstated "),
            "Reject must be double-nested, got {rendered}"
        );
        NotaSource::new(&rendered)
            .parse::<GuardianVerdict>()
            .expect("rendered Reject must parse back");
    }

    #[test]
    fn every_model_reason_is_glossed_and_no_harness_reason_leaks() {
        for reason in MODEL_REASONS {
            assert!(
                reason.admission_gloss().is_some(),
                "model reason {reason:?} must carry a gloss"
            );
        }
        assert_eq!(MODEL_REASONS.len(), 17);
        assert!(
            GuardianRejectionReason::HarnessMalformed
                .admission_gloss()
                .is_none()
        );
    }

    #[test]
    fn assembled_system_prompt_includes_every_file_section() {
        let prompt = GuardianPromptBuilder::intent_guardian_system_prompt();
        for marker in [
            "single admission judge",
            "THE RECORD (Entry)",
            "TYPED JUSTIFICATION",
            "BURDEN OF PROOF",
            "THE CHECKLIST",
            "AFFIRMATIVE GUIDANCE",
            "OPERATION FIT",
            "Approval is a high bar",
            "WORKED EXAMPLES",
        ] {
            assert!(
                prompt.contains(marker),
                "assembled prompt is missing section marker {marker:?} — an include_str! prompt file is empty or mis-wired"
            );
        }
    }

    #[test]
    fn assembled_system_prompt_names_metadata_evidence_and_repair_shapes() {
        let prompt = GuardianPromptBuilder::intent_guardian_system_prompt();
        for marker in [
            "direct psyche declaration naming the Certainty rung supports that declared rung",
            "direct psyche declaration naming the Importance rung supports that declared rung",
            "recurrence, architectural centrality, blast radius, keeps-coming-up, or blocks-other-work",
            "direct psyche declaration naming a Privacy rung supports that declared privacy level",
            "do not admit a sibling fresh Record",
            "remand for Supersede, ChangeRecord, Clarify, Retire, or ResolveClarification",
            "metadata rungs",
        ] {
            assert!(
                prompt.contains(marker),
                "assembled prompt is missing guardian alignment marker {marker:?}"
            );
        }
    }
}
