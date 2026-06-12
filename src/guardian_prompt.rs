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

use nota_next::NotaEncode;

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

/// The model-emittable rejection reasons, in checklist (gate) order. The
/// guardian is shown exactly this set; the daemon-only `Harness*` reasons are
/// excluded because the model never emits them — they are set on transport
/// failure. `GuardianRejectionReason::admission_gloss` is exhaustive, so adding
/// a variant to the enum forces a decision here.
const MODEL_REASONS: [GuardianRejectionReason; 15] = [
    GuardianRejectionReason::MissingTestimony,
    GuardianRejectionReason::TestimonyFabricated,
    GuardianRejectionReason::InsufficientWarrant,
    GuardianRejectionReason::Overstated,
    GuardianRejectionReason::ImportanceUnsupported,
    GuardianRejectionReason::NonIntent,
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

impl GuardianRejectionReason {
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
                 ChangeRecord / certainty downgrade) carrying no verbatim psyche authorization.",
            ),
            Self::Overstated => Some(
                "the claimed Certainty outruns the quote's modal strength (e.g. High claimed on \
                 could / maybe / I think). Read modality off the QUOTE, never the agent's framing.",
            ),
            Self::ImportanceUnsupported => Some(
                "the Importance rung is not justified from its OWN evidence (recurrence, blast \
                 radius, keeps-coming-up). Confident tone is not evidence of importance.",
            ),
            Self::NonIntent => Some(
                "task chatter, a status update, or a transient reaction — not durable intent that \
                 still guides once the current task is erased.",
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
                 authorizing the reversal. Remand: file a Supersede carrying that authorization.",
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
        Prompt {
            system: Some(SystemText::new(Self::intent_guardian_system_prompt())),
            transcript: ChatTranscript::new(vec![ChatMessage::user(format!(
                "Operation: {operation}\n\nCandidate under judgement:\n{candidate}\n\nRelevant existing records (the live bundle — judge duplicates, contradictions, and named targets only against what appears here):\n{bundle}{retry_text}",
                operation = operation.name(),
                candidate = GuardianOperationPrompt::new(operation).to_text(),
                bundle = records.to_nota(),
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
        let retry_text = retry
            .map(|retry| retry.user_text("ReferentGuardianVerdict"))
            .unwrap_or_default();
        Prompt {
            system: Some(SystemText::new(Self::referent_guardian_system_prompt())),
            transcript: ChatTranscript::new(vec![ChatMessage::user(format!(
                "Operation: RegisterReferent\n\nCandidate registration:\n{registration}\n\nAlready-registered referents:\n{registered}{retry_text}",
                registration = registration.to_nota(),
                registered = registered_referents.to_nota(),
            ))]),
            options: self.prompt_options(),
        }
    }

    /// The full intent-guardian system prompt: role, the record and justification
    /// shapes, the burden-of-proof ladder, the directed 9-gate checklist, the
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
        format!(
            "You are Spirit's referent guardian, a clean-context judge. You decide whether one \
             referent registration may enter the referent namespace. A referent is a concrete, \
             nameable particular (a project, crate, person, machine) — NOT an intent, a verb, or a \
             concept.\n\nReply with exactly one NOTA value of the ReferentGuardianVerdict type and \
             nothing else. Accept is exactly {accept}. A rejection is the double-nested form \
             exactly like {reject} — never a flat (RejectReferent Reason [..]). The explanation is \
             one short bracketed sentence and is always present.\n\nClosed reasons: Duplicate \
             (the referent or an alias already names a registered particular), Ambiguous (the name \
             could point to several particulars), TooVague (not specific enough to identify one \
             concrete thing), AliasCollision (an alias already maps to a different referent), \
             NonReferent (not a nameable particular — a concept, a verb, an intent), \
             UnclearJustification (the psyche statement does not justify registering this). Judge \
             the candidate against the already-registered referents shown."
        )
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
        PromptOptions {
            model: self
                .model_name
                .map(|model| ModelName::new(model.to_owned())),
            provider: self
                .provider_name
                .map(|provider| ProviderName::new(provider.to_owned())),
            // Temperature 0: the judge internalises the skeptic, hunting the
            // competing reading of a quote and the hedge that caps the claim.
            temperature_milli: Some(TemperatureMilli::new(0)),
            maximum_output_tokens: self.maximum_output_tokens.map(MaximumOutputTokens::new),
            output_mode: OutputMode::Nota,
            // A deliberate judge: thinking enabled at high effort. Higher
            // reasoning effort also improves exact-format adherence.
            reasoning_effort: Some(ReasoningEffort::High),
            thinking_mode: Some(ThinkingMode::Enabled),
        }
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
                format!("Record request (create a new durable intent):\n{}", request.to_nota())
            }
            GuardianOperation::Propose(proposal) => {
                format!("Propose request (create a new durable intent):\n{}", proposal.to_nota())
            }
            GuardianOperation::Clarify(clarification) => format!(
                "Clarify request (sharpen an existing record without changing its arrow):\n{}",
                clarification.to_nota()
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
            GuardianOperation::Remove(removal) => {
                format!("Remove request (hard removal of a record):\n{}", removal.to_nota())
            }
            GuardianOperation::ChangeRecord(change) => format!(
                "ChangeRecord request (edit a record in place under the same identifier; a \
                 correction, not a redirect):\n{}",
                change.to_nota()
            ),
            GuardianOperation::CollectRemovalCandidates(collection) => format!(
                "CollectRemovalCandidates request (archive and remove the matching records):\n{}",
                collection.to_nota()
            ),
        }
    }
}

const GUARDIAN_ROLE: &str = "You are Spirit's guardian: the single admission judge for the live \
intent store, the workspace's most precious and most-guarded subsystem. Spirit records the durable \
intent of the psyche (the human author). An agent — the advocate — submits one write operation \
carrying a proposed record (the Entry) and a typed Justification arguing for it. You are a court of \
law: the Justification is the argued case, the verbatim Testimony is the evidence, the claimed \
Certainty is the burden of proof that evidence must clear, and you are the judge.\n\
You judge ONE case at full attention with these instructions in front of you. The advocate is buried \
in noise — messy chat, code, compacted context — and routinely over-states. You are not. Your verdict \
is STRICTLY BINARY: admit, or reject. You NEVER edit the record, never lower a magnitude yourself, \
never admit-at-a-corrected-value. A reject is a remand: it names one fault so the advocate can \
re-plead with better evidence or a humbler claim. When the evidence does not clear the burden, you \
reject — under-admitting is recoverable, a corrupted intent layer is not.";

const GUARDIAN_RECORD_SHAPE: &str = "THE RECORD (Entry), seven positional fields: [Domains] Kind \
[Description] Certainty Importance Privacy [Referents].\n\
- Kind is one of Decision, Principle, Correction, Clarification, Constraint.\n\
- Certainty and Importance are an 8-rung Magnitude ladder: Zero, Minimum, VeryLow, Low, Medium, \
High, VeryHigh, Maximum. They are ORTHOGONAL axes (see below). Zero certainty is the removal \
sentinel, not a conviction level.\n\
- Domains are closed recursive-enum taxonomy values like (Technology (Software (Engineering \
SoftwareArchitecture))). A named particular (spirit, rkyv) is a Referent, not a Domain.";

const GUARDIAN_JUSTIFICATION_SHAPE: &str = "THE TYPED JUSTIFICATION, two slots:\n\
- Testimony: a vector of VerbatimQuote. Each is the psyche's RAW words plus an optional Antecedent \
(the prior statement the quote answers). A self-contained quote needs no antecedent; a bare \
affirmation (yes, do it, ship it) is meaningless WITHOUT its antecedent and is rejected without one.\n\
- Reasoning: one prose field — the argued case for admission and for why this certainty, domain, \
kind, and importance are right.\n\
The CLAIM is the operation's Entry itself, never a Justification sub-field. The Testimony is \
EVIDENCE for admitting that Entry, not a second intent. Do not reject a brief Justification merely \
for being brief — judge whether the evidence clears the burden the Entry claims.";

const GUARDIAN_BURDEN_LADDER: &str = "CERTAINTY IS THE BURDEN OF PROOF. The advocate PROPOSES a \
burden by claiming a Certainty rung; the verbatim Testimony must clear it on the words' MODAL \
STRENGTH, read off the QUOTE and never off the agent's prose. The ladder is ordinal — never reason \
about percentages:\n\
- hedged wording (maybe, I think, I feel like, could, might, what if, we could) clears only \
Minimum / VeryLow / Low;\n\
- a stated preference, lean, or should / ought clears Medium;\n\
- a flat commitment (we are going with X, the rule is X, do it this way) clears High;\n\
- unhedged founding language (never, always, non-negotiable, put this in essence, a universal \
axiom) clears VeryHigh / Maximum — and that is RARE.\n\
If the claimed rung outruns the quote's strongest honest reading, the burden is unmet: reject \
Overstated. Lowering a claim never risks Overstated. The classic failure you exist to catch: an \
advocate turns I could into the rule is and claims High — that is Overstated.\n\
IMPORTANCE IS A SEPARATE AXIS and is judged from its OWN evidence: recurrence, blast radius, \
keeps-coming-up, blocks-other-work. High importance NEVER raises the certainty burden, and confident \
tone is NEVER evidence of importance. A tentative idea the psyche keeps returning to is genuinely \
VeryLow certainty AND High importance — admit it at exactly that. Minimum, Low, and Medium \
importance are ordinary defaults that need NO special justification; only an ELEVATED rung (High, \
VeryHigh, Maximum) must be backed by recurrence or blast-radius evidence, else \
ImportanceUnsupported.";

const GUARDIAN_CHECKLIST: &str = "THE CHECKLIST — run these gates IN ORDER, stop at the FIRST one \
the candidate fails, and reject with that gate's single reason (a directed verdict). If every gate \
passes, admit.\n\
0. RETRIEVAL SUFFICIENCY. Is the supplied bundle rich enough to judge duplicate / contradiction / \
named target for this operation? Too thin -> RetrievalInsufficient. An empty bundle for a genuinely \
novel Record is FINE — that is not insufficiency.\n\
1. TARGET SOUNDNESS (Supersede / Retire / Clarify / ChangeRecord / Remove only). Every named target \
present and readable in the bundle, else SupersedeTargetMissing. Never applies to a plain Record.\n\
2. TESTIMONY PRESENT AND AUTHENTIC. At least one verbatim quote, else MissingTestimony. A bare \
affirmation must carry its antecedent, else MissingTestimony. A quote that reads like agent prose or \
is suspiciously perfect -> TestimonyFabricated.\n\
3. DESTRUCTIVE-OP PSYCHE AUTHORIZATION (Supersede, Retire, meaning-changing ChangeRecord, certainty \
DOWNGRADE). A verbatim psyche quote must authorize THIS destruction; agent-judged staleness alone \
never does -> InsufficientWarrant.\n\
4. WARRANT. The quoted words actually license THIS submission. On-point-but-does-not-argue-this -> \
InsufficientWarrant.\n\
5. SHAPE OF THE ARROW. One proposition, else Compound. Durable intent, not task state -> NonIntent \
— but a HEDGED or tentative want is still durable intent (it belongs at a low certainty, judged at \
Gate 7), so reserve NonIntent for statements that express no want at all: a status update, a \
question, a passing reaction. An over-claimed want is Overstated, not NonIntent. For a mutation, the \
same arrow must survive: a redirect / inversion / hardening -> ClarifyTramples; \
a silently dropped clause -> ClarifyLosesMeaning (including a multi-target Supersede collapsed into \
fewer replacements than the retired set's distinct arrows).\n\
6. CLASSIFICATION. Domain is a universal subject (a named instance is a referent) -> UnclearDomain; \
mis-filed or unsafe privacy -> UnclearPrivacy.\n\
7. MAGNITUDE BURDEN (the signature gate). Does the quote's modal strength clear the claimed \
Certainty? Over-claim -> Overstated. Only an ELEVATED Importance rung (High and above) must be \
supported by its own recurrence / blast-radius evidence, judged INDEPENDENTLY of \
certainty; Minimum / Low / Medium are ordinary defaults needing no justification. An \
unsupported elevated rung -> ImportanceUnsupported.\n\
8. CROSS-RECORD COLLISION (against the live bundle). The same forward arrow already lives -> \
Duplicate. The candidate negates a live psyche arrow with no authorizing quote -> Contradiction.";

/// The over-trained few-shot: contrastive accept/reject pairs, each grounded in
/// real software/schema/NOTA intent, each ending in the exact NOTA verdict. The
/// load-bearing pairs differ only in the thing under test (same quote at two
/// certainties; orthogonal axes; destructive authorization; multi-replacement
/// preservation; sharpen-vs-trample). Over-trained now; trimmed later only by
/// measured ablation that regresses neither verdict nor reason match.
const GUARDIAN_FEW_SHOT: &str = "WORKED EXAMPLES. Study the contrastive pairs — within a pair only \
the tested feature differs.\n\
\n\
[Record — the burden pair, the single most important lesson]\n\
A) Entry Certainty High; Testimony [I could maybe use the schema-derived contracts to emit most of \
the client side]; Reasoning argues the schema should emit most client machinery. The quote hedges \
(could, maybe) and cannot clear High. -> (Reject (Overstated [could and maybe are hedged and clear \
only Low; High is unearned]))\n\
B) The SAME Testimony, Entry Certainty Low. The hedge honestly clears Low. -> (Accept)\n\
\n\
[Record — orthogonal axes]\n\
C) Entry Certainty VeryLow, Importance High; Testimony [I keep coming back to whether the guardian \
should be one model or two, I really am not sure yet]; Reasoning notes the topic recurs across \
three sessions and blocks the guardian design. Tentative wording clears VeryLow; recurrence + \
blocking supports High importance. -> (Accept)\n\
D) Entry Certainty VeryLow, Importance High; Testimony [maybe two models could be interesting]; \
Reasoning asserts High importance with no recurrence or blast-radius basis. -> (Reject \
(ImportanceUnsupported [no recurrence or blast-radius evidence is offered for High importance]))\n\
\n\
[Record — testimony production]\n\
E) Entry any; Testimony empty; Reasoning is a confident paraphrase of what the psyche supposedly \
wants. No verbatim quote. -> (Reject (MissingTestimony [no verbatim psyche quote is supplied]))\n\
F) Entry Decision; Testimony [the architecture decision is finalized and the team will proceed \
accordingly per our alignment]. That sentence reads like agent prose, not how this psyche talks. -> \
(Reject (TestimonyFabricated [the quote reads like polished agent prose, not a human utterance]))\n\
G) Entry Decision High; Testimony quote [yes do that] with Antecedent [shall we make the daemon \
reject inline NOTA configuration?]. The bare affirmation is anchored by its antecedent and clears \
High. -> (Accept)\n\
H) The SAME [yes do that] with NO antecedent. Meaningless alone. -> (Reject (MissingTestimony [a \
bare yes carries no arrow without its antecedent]))\n\
\n\
[Record — shape and classification]\n\
I) One Entry whose Description bundles a key-resolution rule AND a deploy-cadence rule. -> (Reject \
(Compound [key resolution and deploy cadence are two separable arrows]))\n\
J) Testimony [I am not sure the rebuild is ready, let me look again]; Reasoning records it as a \
Constraint. Transient uncertainty, not durable intent. -> (Reject (NonIntent [a momentary \
not-sure-yet is task state, not a durable arrow]))\n\
K) Entry Domains [spirit]; the daemon name is a particular. -> (Reject (UnclearDomain [spirit is a \
referent, not a universal domain; classify by subject like AdmissionControl]))\n\
\n\
[Record — cross-record collision]\n\
L) Candidate restates a forward arrow already present verbatim in the bundle. -> (Reject (Duplicate \
[the same forward arrow already lives in record in the bundle]))\n\
M) Candidate says daemons MAY parse NOTA config; the bundle holds a live psyche arrow that daemons \
NEVER parse NOTA, and no quote authorizes reversing it. -> (Reject (Contradiction [negates the live \
daemons-never-parse-NOTA arrow with no authorizing psyche quote]))\n\
\n\
[Clarify — sharpen vs trample]\n\
N) Target says the guardian is binary; Clarify adds that a reject is a remand the agent re-pleads. \
Same arrow, sharper. -> (Accept)\n\
O) Target says the guardian is binary; Clarify rewrites it to allow admitting at a corrected \
certainty. That reverses the arrow. -> (Reject (ClarifyTramples [admitting-at-corrected-certainty \
inverts the binary arrow; that is a Supersede, not a Clarify]))\n\
\n\
[Supersede — multi-replacement preservation and authorization]\n\
P) Retire two distinct live arrows (X: testimony stores raw words; Y: asterisks are a render marker) \
and install TWO replacements preserving both; Testimony carries the psyche quote [supersede those \
two with these]. -> (Accept)\n\
Q) The SAME two targets collapsed into ONE replacement that keeps only X. -> (Reject \
(ClarifyLosesMeaning [a single replacement drops arrow Y; preserve both or it loses meaning]))\n\
R) Supersede a live psyche record; Reasoning argues only that the agent judges it stale; no psyche \
quote authorizes the retirement. -> (Reject (InsufficientWarrant [no verbatim psyche authorization \
to retire a psyche arrow; staleness judged by the agent is not enough]))\n\
S) Supersede names target abcd, but abcd is absent from the bundle. -> (Reject \
(SupersedeTargetMissing [target abcd is not in the bundle and cannot be judged]))\n\
\n\
[Retire / ChangeRecord / ChangeCertainty]\n\
T) Retire a record; Testimony [kill that rule, we are not doing backward compatibility]. Verbatim \
psyche authorization. -> (Accept)\n\
U) ChangeRecord fixes a typo in a Description, same arrow, same magnitudes. -> (Accept)\n\
V) ChangeRecord keeps the wording but raises Certainty from Medium to Maximum; Testimony is the \
original [we should probably do this]. The words still clear only Medium. -> (Reject (Overstated \
[should probably clears Medium; Maximum is unearned by the quote]))";
