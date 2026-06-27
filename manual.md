# The Spirit Manual

Spirit is the intent layer. It captures what the psyche — the human author —
directs, decides, and wants, and serves that intent back on query. This manual
explains what Spirit holds, how a statement becomes a record, how the guardian
gate decides admission, how certainty and importance are set, how records age
out, and the everyday CLI conventions.

This is the manual, not the store. Spirit holds durable intent; the explanation
of how Spirit works lives here.

## 1. What Spirit Is

Intent logging is the most important part of the system and dwarfs everything
else around it. Recorded intent is precious, tightly controlled data — the
center that the rest of the design exists to serve. Everything else (the CLI
edge, the daemon, the store, the schema machinery) is in service of keeping
that intent layer clean, queryable, and true to the psyche's direction.

Because the data is precious, judgment is concentrated in one place. The Spirit
guardian is the single locus of semantic judgment: the model checks everything
that requires meaning — consistency, duplication, trampling of existing intent,
and whether a submission is genuine intent at all. Only structural admission
(does the message parse and typecheck) stays upstream as ordinary input
validation. There is no second semantic gate scattered elsewhere; what needs
understanding is understood once, at the guardian.

## 2. What Spirit Holds: Intent, Not Information

Spirit stores only durable psyche statements — the things that still guide
after the task that produced them is gone. The test for every candidate is
simple: would this record still guide once its originating task is done? If
yes, it may belong in Spirit; if it only describes the current task, it does
not.

Five shapes are recordable:

- **Decision** — a settled choice ("we are going with X, not Y").
- **Principle** — a general rule ("X over Y, as a rule").
- **Correction** — a fix to a prior belief ("you were wrong about X; it is Y").
- **Clarification** — a sharpening of meaning ("when I said X, I meant Y").
- **Constraint** — a boundary that holds ("never do Z", written as the
  affirmative rule it protects — see section 4).

Intent is not information. The intent layer holds intent — what the psyche
directs, decides, or wants — not facts or beliefs. A statement can sound
durable and still carry no direction behind it; if there is no arrow, it is
information, not intent, and it is not captured. Working orders are the common
trap: "create the report," "dispatch a subagent," "audit X," "integrate the
branches" are task state, not durable intent. When a task prompt also carries a
durable arrow inside it, capture only that arrow and let the order itself live
in the task surface. A working order that slips into the log is flagged as a
capture error, not silently deleted.

When a design surface is incomplete, the discipline is to ask the psyche rather
than generate a plausible synthesis and capture it as if the psyche had
authorized it. Inferring to close the loop manufactures fake, hallucinated
records — exactly the corruption the intent layer cannot tolerate. Ask; do not
infer.

## 3. The Capture Flow

The right answer to "too much was captured" is not to stop using Spirit. The
discipline is the conservative capture gate: use Spirit only for explicit,
durable decisions, principles, corrections, clarifications, and constraints,
and let everything else stay out. Avoiding Spirit entirely is as wrong as
over-capturing it.

**Refresh intent first.** Before maintenance or implementation, refresh intent
— which means the agent reads recent Spirit records to refresh its own working
context. "Refresh intent" is a read, not a write: it does not mean editing a
repo's `INTENT.md` or architecture files. Those file edits happen only when the
psyche explicitly asks for them.

**Classify, then act.** A candidate statement resolves to one of: no capture
(a question, tangent, task-only order, or current-state reaction with no
durable rule); observe (read existing records for context); ask (the durable
meaning, kind, or privacy is unclear); edit an existing record (the psyche is
clarifying or narrowing something already captured); or record (explicit
durable intent that passes the gate). No-capture is the normal outcome.
Understatement is recoverable later; over-extension corrupts the load-bearing
layer.

**One capturer per prompt.** When a single prompt addresses more than one lane,
exactly one lane records the intent — by default the first responder. In
practice the operator lane (Codex) responds faster than the designer lane
(Claude), so the operator usually writes the Spirit entry; the slower lane then
gap-checks the capture and fills only a genuine omission rather than writing a
parallel record. Both lanes engaging with the substance is correct; both lanes
logging the same record is the duplicate failure this rule prevents.

## 4. The Guardian Gate

Capture is a blocking gate, not an advisory check. The guardian vets and admits
a proposed record before capture can succeed; duplicates, contradictions,
compound entries, and non-intent are resolved or refused at the door rather
than cleaned up afterward.

**Semantic judgment, deterministic plumbing.** The guardian performs the
semantic judgment on every guarded write, while deterministic code does the
mechanical work around it: gathering context, validating structure, and
applying the typed consequences of a verdict. The guardian judges a submission
across the dimensions its justification must satisfy — does the entry justify a
genuine intent, is the domain correct, do the certainty and importance fit the
evidence — and it judges a cross-record operation as one whole, a single
yes/no. A supersede that atomically retires a set and installs several new
intents is checked together: the guardian confirms the new set as a whole still
preserves the kernel of what it replaces.

Each operation type carries its own per-operation prompt with worked accept and
reject examples (over-trained first, then scaled back), and the guardian always
returns well-formed output through a generated verdict grammar with a closed
rejection set, parse-and-retry, and a shape test. A clean-context, specialized
guardian outperforms the submitting agent even when it runs on a weaker model,
precisely because its context is uncluttered and its job is narrow.

**Atomic accept or reject, including referents.** An entry and the new
referents it introduces are decided together as one accept/reject, so no orphan
referent survives a refused entry; referent registration is gated exactly like
records. Intent-resolution operations are typed, combined actions the guardian
resolves in a single atomic call — for example, adding a record while
deprecating named records by identifier in the same move.

**Judged against the psyche's actual words.** The guardian judges a proposed
change against the verbatim words of the psyche and their context, not only
against the existing records. Testimony must be the psyche's exact words; a
paraphrase is rejected. A bare affirmation like "yes" or "okay" carries no
meaning alone, so it must travel with the statement it was answering — the
question or context is part of the evidence.

**Affirmative framing.** The guardian admits captures whose operative guidance
states the affirmative shape to follow — what the practice is, what the
canonical name or contract is, what boundary holds. A capture framed primarily
as an exclusion, prohibition, or definition-by-negation is returned to the
submitting agent for positive rewording before admission. This applies even to
a Constraint: write the boundary as the affirmative rule it protects, not as a
bare "never."

**Rejections are remands.** A guardian rejection is not a dead end; it names
the coherent repair shape. When a proposal is not admissible as written, the
guardian identifies the operation family or maintenance path that would make
the intent change coherent — reword it affirmatively, clarify an existing
record, change the target record, or supersede the records it conflicts with.

**Psyche declarations are primary evidence.** When the psyche explicitly
declares an intent's value or metadata, the guardian treats that declaration as
primary evidence for the declared value. If such a declaration conflicts with
active records, the guardian does not simply refuse — it returns the submission
to the agent as a broader maintenance edit that resolves the conflicting
records (clarify, supersede, retire, change, or remove) within the same intent
change, so the active store stays coherent around the psyche's current
direction.

## 5. Certainty and Importance

Every entry carries exactly two orthogonal axes, and neither substitutes for
the other:

- **Certainty** — confidence that the statement is true / in force.
- **Importance** — how much the statement matters.

They are never conflated. A tentative idea can be very-low certainty yet high
importance. Importance (formerly called Weight) is declared directly by the
psyche, not only accumulated through repetition — a statement made once can
carry high importance. There is no separate Weight field; importance is the
axis.

**The certainty rubric.** Certainty follows an honest eight-level magnitude
ladder, assigned deliberately and never reflexively:

- **Maximum** — foundational invariants and hard overrides, repeatedly
  ratified and stable. Genuinely rare.
- **VeryHigh** — ratified decisions clearly in force.
- **High** — solid decisions with only minor open edges.
- **Medium** — working decisions and design directions still settling. This is
  the honest home for most records — the place where agents once reflexively
  marked Maximum.
- **Low / VeryLow** — leans, proposals, and explicitly-pending information.
- **Minimum** — weak, tentative signal that might matter later.
- **Zero** — not a confidence level at all; the removal-candidate marker (see
  section 6).

Certainty is chosen from how sure the psyche actually sounded, not from how
important the topic is. Under-rating is recoverable; over-rating corrupts the
signal.

**Declaration plus argued evidence.** Metadata rungs can be set two ways. When
the psyche explicitly names a certainty, importance, or privacy rung, that rung
is supported by the testimony itself. The submitting agent may also argue for a
rung from evidence — repeated psyche emphasis, architectural centrality, blast
radius — and may even argue for a higher rung than the psyche named when the
evidence genuinely supports it. The guardian weighs the case and rejects only
when the claimed rung exceeds both the explicit testimony and the argued
evidence.

**Agglomeration does not raise certainty.** When repeated or related intent is
agglomerated, the merge preserves source provenance and may preserve
accumulated importance (through the weight axis where it exists), but it does
not automatically raise certainty. Certainty rises only when the synthesized
statement is itself better supported, or stated with higher confidence, than
its parts were.

## 6. Record Lifecycle

Records do not just accumulate; they are clarified, superseded, retired, and
eventually removed as the psyche's direction moves.

**Zero certainty is the removal-candidate state.** A Zero record has no value
and must not surface by default. Setting certainty to Zero nominates a record
for removal while keeping it recoverable: change the certainty back to a
non-zero magnitude and it returns. This unifies the removal-candidate concept
with zero certainty — there is no separate flag.

**Edit operations preserve coherence.** When the psyche refines existing
intent, the right move is an edit, not a second record that future readers must
reconcile with the first. Clarify edits one record's description while keeping
its identity. Supersede atomically retires one or more old records and installs
their replacements, checked as a whole by the guardian. Retire deactivates a
record without a replacement. Change rewrites a record's entry. Physical
deletion is not a working verb: the owner archives-then-removes matching records
through the meta `CollectRemovalCandidates` operation, used after review when
nothing should remain in the live log. Because conflicting psyche declarations
are remanded as maintenance edits (section 4), the active store is kept coherent
around current direction rather than layered with contradictions.

**Maintenance through a Spirit subagent.** Intent-led orchestration and
grilling leads periodically dispatch a dedicated Spirit-maintenance subagent to
handle psyche answers. That worker first inspects or searches the relevant
Spirit domain and referent records, then classifies each answer as a
clarification, a supersession, a new record, or non-Spirit task material —
routing each to the correct operation instead of reflexively minting new
records.

**Intent-clarity audits cross-pollinate.** Intent gaps are filled by
cross-pollinating patterns already present elsewhere in the intent: when one
area lacks a clear design, an analogous pattern established in another area is
projected onto the gap. This surfaces broken or implicit conventions and turns
them into stated principles, and is the working method during intent-clarity
audits of recent work.

## 7. Querying and Observing

The default query view hides removal candidates. The ordinary Observe query
excludes Zero-certainty records, because a Zero record has no value to a normal
read. Queries take a certainty floor that defaults to Minimum (which excludes
Zero); to see Zero records you must set the floor explicitly to Zero or Any.
Lookup by identifier bypasses the observation filters, so a known Zero record
can still be read directly when you already hold its id.

Beyond the certainty floor, Observe selects on domain, keyword, free text,
referent, kind, privacy, and importance, so a reader can ask for exactly the
slice of intent they need. Referents — the named particulars a record is about
— are the primary retrieval and dedup key; the guardian itself pulls existing
records that share a referent with a candidate, which is why entries are tagged
with the real particulars they concern.

## 8. CLI Basics and Reply Conventions

A handful of conventions govern how the CLI and its replies behave:

- **Version is a bare selector.** A versioned CLI needs a way to ask which
  version is active. Spirit exposes this as the bare NOTA input `Version`
  (`spirit Version`) — not a Unix-style flag and not a parenthesized empty
  record.
- **Identifiers are shown as shortest unique prefixes.** A record-acceptance
  reply displays the shortest collision-free lowercase identifier prefix, with
  a minimum length of four characters, rather than the full stored identifier.
  Cite and pass that short code.
- **Replies stay clean of markers by default.** Ordinary agent-facing replies
  do not include database markers. The durable database marker is returned only
  by an explicit operation that asks for marker state.
- **Timestamps are daemon-stamped.** Clients never supply a time; the Spirit
  daemon stamps each record. There is no client time field on an entry.

These conventions keep the everyday surface terse: a write returns a short
identifier and nothing the reader did not ask for, and the machinery (markers,
timestamps, full identifiers) stays available behind explicit requests.
