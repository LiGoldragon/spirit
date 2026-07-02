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

Spirit is a universal intent tool for every human, not bespoke to one psyche or
workspace. Its overriding design goal is to stay maximally clutter-free — a
curated, pristine intent store rather than a capture-everything log. Every
discipline in this manual exists to protect that goal.

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

**What intent is.** Intent is the rare, orienting will of the psyche — what he
steers toward (an aim), holds as worth (a value), or fundamentally believes. It
is not a decision, default, wish, or rule; those are matter. It has a magnetic,
unbending quality: he holds it even against his own convenience, and it bends
many downstream choices toward it like a North Star. It is often hard to fully
verbalize. Capture is the exception, not the reflex; when unsure, it is not
intent — ask, don't infer.

Intent is not information, and it is not matter. A statement can sound durable
and still carry no orienting will behind it; a routine, a default, a mechanism,
or a rule about how the tooling itself works is matter, and matter belongs in
code, docs, the tracker, or a skill — never in the intent log.

**The five-gate test.** Capture a statement as intent only when all five hold.
Any miss makes it matter:

1. **Aim, value, or belief** — not a how, a default, a mechanism, or a rule.
2. **Unbending** — he would hold it against cost or convenience; "for the
   spirit, not for profit."
3. **Orienting** — it bends a whole class of future decisions, not one local
   case.
4. **Its "why" is a value** — it bottoms out in what he wants, not an
   engineering or efficiency tradeoff.
5. **From the psyche, felt** — not agent-synthesized to close a loop.

**Don't be fooled by** rule-grammar (must / never / always); a "why" that is
only an engineering justification; vivid or eloquent phrasing; a sensible
one-off default; or agent-procedure and Spirit-operation procedure.

**Worked example.** "New repositories default to public" reads like a rule, but
it fails gates 1, 2, 3, and 4: it is a how/default, reversible for convenience,
local rather than orienting, and justified by an ordinary engineering tradeoff.
It is matter, not intent.

Working orders are the common matter trap: "create the report," "dispatch a
subagent," "audit X," "integrate the branches" are task state, not durable
intent. When a task prompt also carries a durable arrow inside it, capture only
that arrow and let the order itself live in the task surface. A working order
that slips into the log is flagged as a capture error, not silently deleted.

When a design surface is incomplete, the discipline is to ask the psyche rather
than generate a plausible synthesis and capture it as if the psyche had
authorized it. Inferring to close the loop manufactures fake, hallucinated
records — exactly the corruption the intent layer cannot tolerate. Ask; do not
infer.

### The grounding: Castaneda's "intent" and the psyche's felt-sense

This definition is not invented. It recovers what the psyche has always meant by
intent, and it matches Carlos Castaneda's use of the word almost line for line.
The psyche describes intent as "an aura of astral divine magnetism," "the je ne
sais quoi that gives life the fire of life," "the glitter behind the beholder's
eye," "the force that makes a man do incredible things through extraordinary
perseverance and dedication," and "the North Star." Castaneda's don-Juan texts
name the same force on every axis:

- **Magnetism, the pull you cannot argue away.** "I couldn't possibly extricate
  myself from the magnetic pull that the intent of those shamans had created. I
  was drowning in it, whether or not I believed in it or wished for it." (*The
  Wheel of Time*, commentary on *A Separate Reality*.) This is the "aura of
  astral divine magnetism," and it is gate 2: intent is held against one's own
  reasoning and wish.
- **Carrying the actor past his own defeat.** "Intent is not a thought, or an
  object, or a wish. Intent is what can make a man succeed when his thoughts
  tell him that he is defeated. It operates independent of any warrior's
  indulgence. Intent is what makes him invulnerable." (*A Separate Reality*.)
  This is "the force that makes a man do incredible things through extraordinary
  perseverance."
- **For the spirit, not for profit.** "Warriors have an ulterior purpose for
  their acts, which has nothing to do with personal gain. The average man acts
  only if there is the chance for profit. Warriors act not for profit, but for
  the spirit." (*The Power of Silence*.) A reversible default chosen for
  convenience or expected return is, in exactly these terms, the ordinary/profit
  side — matter — not a gesture of the spirit.
- **Felt and near-ineffable.** Understanding intent "cannot be turned into words
  ... there to be felt, to be used, but not to be explained"; Castaneda's reason
  briefly mistook it for God and was corrected — "it could not possibly be God,
  because intent was a force that could not be described." This is "the je ne
  sais quoi" and "the glitter behind the beholder's eye," and it is why the
  definition says intent is often hard to fully verbalize.
- **Orienting, the North Star.** "In the universe there is an immeasurable,
  indescribable force which shamans call intent, and absolutely everything that
  exists in the entire cosmos is attached to intent by a connecting link." (*The
  Power of Silence*.) Intent orients a whole class of choices toward it; that
  connecting link is the psyche's North Star.

Castaneda draws the same intent/matter line the capture gate draws. The "average
man's connecting link with intent is practically dead," numbed by "the ordinary
concerns of ... everyday life"; ordinary behavior is routine, transactional, and
reversible by preference. Reviving intent takes "a rigorous, fierce purpose — a
special state of mind called unbending intent." Mechanical, routine, convenient
defaults are precisely the numbing everyday concern; true intent is the single,
sustained, magnetic purpose held with impeccability. That is why the gate is
strict and capture is rare.

### Why the word "intent" stays

A survey of alternatives — will, true will, daimon, lodestar, telos, calling,
entelechy, numen — found none that wins both tests a replacement must pass: the
felt-sense of the force, and everyday usability as a capture test. The luminous
words name a force or a sacred quality rather than a capturable directive;
daimon is the strongest felt-sense match but collides fatally with the computing
daemons this stack runs on; lodestar is the serendipitous near-miss that fuses
the psyche's own "North Star" and "magnetism" images but names the guide one
steers by, not the will itself. "Intent" is a clean directive noun that already
serves as the capture test, and its very soberness enforces the high bar: a more
luminous word would invite agents to feel fire everywhere and over-capture,
which is backwards. A rename is opportunistic only, the blast radius is large,
and nothing clears the bar. Intent stays.

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

**Observe is routine, not a flag-triggered exception.** Running a Spirit Observe
is standard practice, not optional. Agents observe recent records as a matter of
course — proactively at the start of and during substantive work — to let
recorded intent guide the work, rather than only when the guardian flags intent
as unclear. Treat it as a working guide you read yourself, not an open decision
to present to the psyche.

**Educate yourself in the domain before submitting.** An agent reads the
domain's existing intent before it records, proposes, clarifies, or supersedes:
the candidate's domain and the broader domains above it, plus records sharing
the candidate's referents. Done well, most submissions resolve to a duplicate, a
merge, or a clarify rather than a guardian refusal — the guardian is the
fallback, not the agent's first read. A subagent that needs to understand a
domain, referent, or unknown named thing begins by querying Spirit; when the
exact referent is unknown, it searches public text over the relevant terms
before relying on local inference.

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

Concurrent capture of the same prompt across separate agent windows is handled
by a lock, so two agents do not both record one intent. Capture calls are
asynchronous and non-blocking — a lane that loses the lock is not stalled; it
later receives a reply naming which records were accepted or refused during the
lock and why, recognizes the duplicate as coming from the same prompt, and
either accepts it or argues for a better wording. This depends on agents
recording their own originating prompt so Spirit can deduplicate captures by
prompt.

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

The judge is not under-resourced. Intent is too important, so the gate runs a
strong model (open-weight where Spirit is self-hosted). Relevance, duplication,
and contradiction are model-only semantic judgments, so the judge sees the full
evidence bundle; relevance is computed once at write time and stored as a
referent for cheap scoped reuse, bounded by referent and domain scoping. A Spirit
with no configured guardian fails closed — it refuses rather than admitting an
ungated write.

**Capture is a court of law.** The submitting agent advocates, the psyche's
verbatim quotes (asterisk-bracketed spans) are testimony, and the guardian is
the judge rendering a binary verdict. The justification is a strongly-typed
argued case — typed testimony with optional antecedents plus reasoning for why
the certainty, domain, kind, and importance fit — never a stringly blob; the
claim itself is the operation's own entry. Certainty is the burden of proof the
quotes must clear on modal strength: an over-claimed certainty is rejected back
to the agent to reword, and the guardian never silently lowers it, preserving
the psyche's modality. A bare affirmation like "yes" carries no meaning alone, so
it must travel with the statement it answered.

**Atomic accept or reject, including referents.** An entry and the new
referents it introduces are decided together as one accept/reject, so no orphan
referent survives a refused entry; referent registration is gated exactly like
records. Intent-resolution operations are typed, combined actions the guardian
resolves in a single atomic call — for example, adding a record while
deprecating named records by identifier in the same move. The guardian is not
called for a referent that is already registered: a registration request naming
an existing canonical referent is settled by the registry from existing state,
with no model judgment. Only a new alias or new referent is a real registry
change that stays gated.

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
its parts were. If a separate weight axis is ever added to preserve accumulated
importance after source mentions vanish, it uses the same qualitative Magnitude
ladder (Zero through Maximum) on its own axis distinct from certainty — never an
integer count — keeping the whole contract qualitative.

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

**Provenance through a relations field.** Provenance and agglomeration are
carried by a relations field on a record — a vector of short non-colliding
record-id references pointing to source or related records (a Correction relates
to what it corrects) — not a dedicated Composite record kind. This lets repeated
or related intent be merged into a newer, stronger record without losing
provenance; refreshing several intents may yield one merged record or two or
three records of different kinds. The refreshing judgment itself is agent
behavior trained through the intent-maintenance skill, not engine logic: the
relations field is the only supporting machinery, and agents learn to refresh
many related intents into fewer stronger records.

**Removal is psyche-authorized and conservative.** Deployed Spirit can remove
records, superseding the old append-only / flag-only constraint, but every
removal needs a justifying psyche statement (a changed mind is enough; no
replacement arrow is required for a deletion). Clean the log by removing records
a newer record, `ARCHITECTURE.md`, or skill has absorbed, and working orders
that fail the after-the-task test. When removability is uncertain, flag rather
than remove: over-removal is worse than under-removal. An automated auditor can
auto-propose refreshes and surface low-confidence records by magnitude, but the
psyche confirms the retire of source records — automated discovery, human-gated
removal. The in-place mutation path edits a record's content rather than
removing and recreating it, so identity and provenance survive an edit; hard
deletion remains a deliberate planned decision on a long timeline, not a
forbidden state.

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

`All` is a complete leaf domain value available at every level of the domain
tree, meaning all alternatives at that level. It is symmetric across querying and
assignment: a record may be tagged `All` at any level, and a query may request
`All` at any level. `All` is the explicit name for an early stop — the
all-of-`Software` value is written by stopping at `All` under `Software` — so the
representation is unified rather than carrying an implicit optional stop. A
domain-based query returns, alongside the specific matches, the `All`-tagged
records of every parent level along the queried path, so the top-level `All`
maxims surface for any specific query; this ancestor-`All` inclusion is a
configurable shorthand, with a plain mode that does not fold the parent records
in. A registered but undelegated domain name returns a typed no-records result
rather than an error.

For ordinary public search the `PublicTextSearch` verb is the short path: it
takes one text payload and ranks active public records by description and
referent text, instead of forcing agents through the full eight-field `Observe`
query. Agents can also run a catch-up query from a recorded time to retrieve
intent added since their last read, without depending on numeric identifier
order.

## 8. CLI Basics and Reply Conventions

A handful of conventions govern how the CLI and its replies behave:

- **Invocations default to inline NOTA.** A Spirit call passes its argument as
  inline NOTA wrapped in shell double quotes; the bracket-string notation keeps
  the NOTA itself double-quote-free so the whole object passes as one shell
  argument. A temp-file NOTA path is reserved for genuine need — binary
  signal-encoded paths, or shell metacharacters too painful to escape — not for
  ordinary multi-line context fields.
- **Version is a bare selector.** A versioned CLI needs a way to ask which
  version is active. Spirit exposes this as the bare NOTA input `Version`
  (`spirit Version`) — not a Unix-style flag and not a parenthesized empty
  record.
- **Identifiers are shown as shortest unique prefixes.** A record-acceptance
  reply displays the shortest collision-free lowercase identifier prefix, with
  a minimum length of four characters, rather than the full stored identifier.
  Cite and pass that short code.
- **Writes acknowledge cheaply.** A creation returns only the new record's short
  identifier, not a receipt bundling the identifier with a database marker, and
  an acknowledgement never echoes the submitted intent content back.
- **Replies stay clean of markers by default.** Ordinary agent-facing replies
  do not include database markers. The durable database marker is returned only
  by an explicit operation that asks for marker state.
- **Outcomes are typed NOTA, not status prose.** An operation reports its result
  with self-describing NOTA enums and structs wherever the state can be typed
  data, rather than a long free-text status message.
- **Timestamps are daemon-stamped.** Clients never supply a time; the Spirit
  daemon stamps each record. There is no client time field on an entry.

These conventions keep the everyday surface terse: a write returns a short
identifier and nothing the reader did not ask for, and the machinery (markers,
timestamps, full identifiers) stays available behind explicit requests.

**Short-form operations for the common path.** `Record` stays the canonical
full-fidelity write, but a `RecordDefault` short form takes only the
commonly-customized fields (topics, kind, description, magnitude) and injects
defaults for the rest — privacy `Zero` (open/public), daemon-stamped time. A
named private short form (such as `RecordPrivate`) lowers to a normal record with
an elevated privacy Magnitude, making private capture one deliberate,
unmissable ritual while ordinary shorthands stay public by default. Privacy of an
existing record can be moved in place through a `ChangePrivacy` operation that
mirrors `ChangeCertainty`, preserving identifier and timestamp instead of
forcing a remove-and-re-record. Content-extracting operations such as
`CollectRemovalCandidates` take a customizable output-target enum as their final
field — `Stdout`, `Stderr`, or `File(path)`, where `Stderr` is a normal output
option, not an error channel — so the wire surface stays uniform across present
and future export operations.

Records themselves stay dense. A record carries one clarified description and no
verbatim field; capture preserves the clarified intent without large verbatim
blocks that bloat output and become lossy to work with.

## 9. Topics and Domains

Topics are user-creatable single strings — broad atomic single-word concepts,
not compound hyphenated phrases. Prefer two topics (`intent logging`) over one
glued phrase (`intent-log`), and let a multi-concept record carry several topic
words. Reuse an existing topic word when it covers the substance; invent a new
one only when none fits, so records stay discoverable by either concept and the
vocabulary does not explode into near-synonyms.

Domains are the broad-routing layer, a curated and openly growing vocabulary
that may reach hundreds of specific grounded domains rather than a small fixed
abstract set. The closed domain taxonomy carries only the broad routing;
fine-grained specificity belongs to the open referent layer of named particulars
and to free-text description keywords, not to ever-deeper enum leaves. A taxonomy
leaf earns its place only when it disambiguates and carries real routing load; an
over-specified leaf that behaves like a referent or keyword is cut rather than
kept.

## 10. Record Identity

A record's identity is a stable, non-reusable, opaque random handle assigned once
at creation and frozen — not a content-address fingerprint (records mutate, so
content-addressing would imply stable-content semantics that no longer hold) and
not a reusable incremental number. Concretely it is a 96-bit CSPRNG value
rendered as a lowercase base36 shortest-unique-prefix code. The full random value
stays binary on the wire; only the short prefix appears in text, scoped per kind
and extended only on a same-kind collision, with a minimum displayed length of
four characters (about four to seven), aligning with Beads practice.

Identifiers must never be reused after removal: reuse makes references unstable,
because a later record could occupy a freed identifier and silently change what
an old reference points to. Recency is tracked by daemon-stamped time, not by the
identifier.

## 11. Citing Intent in Prose

Cite a Spirit record in prose by quoting its description summary literally as
bracketed text plus a `(Spirit Kind short)` parenthetical — the bracketed
substance is the citation, not the opaque code. Quote central intents literally
and in a prominent place, especially in psyche-facing reports. This applies to
all agents.

## 12. The Skill Surface and Agent Practice

The Spirit skill has two halves with different sources. This manual half — what
Spirit and intent are, the CLI and wire shape, how to read and query — is
generated from the spirit repository's production-versioned documentation, so the
read-side skill tracks the deployed component instead of being a hand-maintained
duplicate, and the manual is never copied into the intent database. The capture
half — the gate, the certainty and importance ladder, affirmative framing, when
not to record, maintenance — stays primary-authored agent-conduct teaching beside
the other behaviour skills. The Spirit-facing skills document the CLI thoroughly
(invocation, every operation, the deployed wire shape and how to find it when
source drifts, error replies, environment variables) so agents do not scramble or
call stale versioned wrappers.

A few practice rules follow from this:

- **Use the unsuffixed `spirit` CLI for normal capture.** Versioned Spirit
  wrappers are diagnostic or explicit testing surfaces, not the everyday agent
  command.
- **Track the deployed wire interface, not current source.** When two stacks
  coexist, the deployed pinning is reachable through the CriomOS Home flake
  input — use Nix metadata commands (flake metadata, derivation show, path-info)
  to find the pinned commit, then read the wire contract at that revision.
  Operator changes to the source drift it from production until the next rebuild.
- **Keep intent-logging guidance fresh in context.** Psyche-facing agents must
  load the intent guidance before using Spirit; using the tool requires the
  guidance loaded first.
- **Intent-led orchestration centers Spirit as durable memory.** Current
  reporting protocols stay in force where required, but reports are not presented
  as the future durable-memory layer — Spirit is the durable intent memory.
