# INTENT — spirit

`spirit` is the production Spirit daemon and proves a running component can be
built from schema-derived interfaces. It is the current copyable triad runtime
exemplar for the next Spirit engine stack: the ordinary public signal/domain
contract lives in the `signal-spirit` contract crate, the owner-only
meta-policy signal contract lives in the `meta-signal-spirit` contract crate,
while this daemon crate keeps crate-local `schema/nexus.schema` and
`schema/sema.schema`, shared build-driver generation, generated daemon runtime
modules under `spirit::schema`, `sema-engine` storage, and `triad-runtime`
runner/listener/runtime support. It must not be
described as an all-in-one pilot whose shape future components should avoid.
(Spirit record `y88n`, High certainty.)

Load-bearing constraints:

*CLI input and output are NOTA when the `nota-text` feature is enabled.* Component/process communication is always binary rkyv. Generated schema datatypes always carry rkyv support; NOTA encode/decode is an opt-in text-client surface, not a daemon requirement. The daemon binary must not depend on `nota-next`; the CLI crate enables `nota-text`. Tests run `cargo tree --edges normal --no-default-features` and assert `nota-next` is absent from the binary, while the text surface must contain it.

*Rust data types are generated from schema source, with the public signal
contract imported from `signal-spirit`.* Authored schema source is a typed
artifact before Rust emission. `signal-spirit` generates the ordinary
`signal::Input` / `signal::Output` contract and recursive domain tree from its
own `schema/signal.schema` and `schema/domain.schema`. Spirit's shared
generation driver imports that dependency schema, reads the daemon-local Nexus,
SEMA, and meta-signal schemas into `SchemaSource`, round-trips canonical source
text and rkyv archive bytes through `SchemaSourceArtifact`, lowers from that
typed source value to semantic `Schema`, and compares only the daemon-local
generated Rust artifacts with checked-in files. The source language has an
in/out codec instead of being a one-way parser, and `.asschema` is no longer a
checked component artifact.

*Schema namespaces are strict NOTA key-value maps.* Braces are key-value pairs. A namespace entry is a pair like `Topic String`, `Entry { Topics * Kind * ... }`, or `Kind [...]`. Struct fields are key-value pairs; `Topics *` reuses the same type, while `kind (Optional Kind)` binds a field to a different reference. Root enum bodies are square-bracket lists of exported object names. Namespace enum bodies use bare names for unit variants, self-tagged `(Variant)` entries when the payload type has the same name, and explicit `(Variant Payload)` entries only when the payload name differs. Namespace bindings such as `Record RecordRequest`, `RecordAccepted RecordIdentifier`, and `SignalArrived Input` define the payload aliases those signatures reference. Bare bindings lower to aliases and direct enum payloads, not wrapper structs.

*The three runtime centers are concrete objects.* `SignalAdmission` handles admission, `Nexus` is the mail keeper and translator owning the store and ledger, and `Store` is the durable SEMA plane over `sema-engine`. `Engine` composes them and owns no SEMA state. The imported ordinary contract exposes `signal::Input`/`signal::Output`; the daemon-local generated plane namespaces expose `nexus::Work`/`nexus::Action` and `sema::WriteInput`/`sema::WriteOutput`/`sema::ReadInput`/`sema::ReadOutput`.

*Signal admission is explicit.* `SignalAdmission::admit` mints the origin route, validates generated `Input`, and creates `SignalAccepted`. Invalid input returns `Output::Rejected(SignalRejection(ValidationError))` where `ValidationError` is generated from the signal-spirit contract; the runtime does not use a hand-written rejection enum.

*Version is a NOTA-native Signal operation.* Per Spirit record `x5b7` (High
certainty), the CLI version query is the bare NOTA input `Version`, invoked as
`spirit Version`; it is not a Unix flag and not a parenthesized empty record.
Nexus replies directly with generated `Output::VersionReported(VersionReport(VersionText))`, and the version text comes from the component package version.

*Ordinary agent-facing replies do not carry database markers.* Per Spirit
record `8jtz`, `DatabaseMarker` is explicit introspection state, not context
for every agent-facing reply. `Lookup`, `Version`, `Count`, `Observe`,
mutation receipts, guardian rejections, referent registrations, errors, and
stream events omit marker tuples. Callers ask for marker state through the
dedicated bare `Marker` input, which replies `MarkerReported(DatabaseMarker)`.
The marker tuple is `(CommitSequence, StateDigest)`: persisted database
sequence plus a state digest/fingerprint.

*Intent streaming is the first subscription pilot.* Per Spirit record `ubgg`
(Medium certainty), the first streaming proof is agent-facing intent
subscription through the Spirit CLI: `Input::SubscribeIntent(Query)` opens
`IntentEventStream`, returns `Output::SubscriptionStarted(IntentSubscription)`,
and keeps the client attached for daemon-pushed `Output::Event(IntentEvent)`
frames. The stream filters with the same generated `Query` noun as ordinary
observation, and pushed `IntentRecorded` events carry the recorded `Entry` plus
the `RecordIdentifier`. The low-level subscription frame is generated from
schema and built through `signal-frame`/`triad-runtime`; Spirit owns only the
component filter and delivery policy.

*State is the first production-surface compatibility operation.* Generated
`Input::State(Statement)` carries raw psyche text. Signal only admits and
validates the non-empty statement. Nexus exposes classification as the
schema-declared `ClassifyState` effect command and `StateClassified` effect
result; the hand-written policy only implements that declared Nexus interface.
The classified `RecordRequest` carries the generated `Entry` plus a
`Justification` made from the original psyche statement, uses the production fallback policy (`unclassified`,
`Clarification`, `Minimum`, `Zero`) and SEMA persists it through the existing
`Record` write root. The generated canonical NOTA shape is `(State [text])`.
The CLI text edge also accepts deployed production shorthand `(State ([text]))`
and normalizes it to the generated input before binary framing.

*Record mutation parity is schema-visible.* Generated
`Input::ChangeCertainty(CertaintyChange)` keeps the production-facing word
`Certainty` as an alias of the current `Magnitude` scale, and generated
`Input::ChangeRecord(RecordChange)` replaces a stored entry under the same
identifier. Nexus exposes both operations as schema-declared
`CommandSemaWrite(ChangeCertainty)` and `CommandSemaWrite(ChangeRecord)` objects
instead of hidden branches. SEMA applies both as keyed mutations, preserving
the existing `RecordIdentifier`; certainty changes mutate only the stored
entry's certainty, while record changes replace the full stored
`Entry`. Replies carry the updated `CertaintyChangeReceipt` or `RecordChangeReceipt`; callers use `Marker` when they need the database marker.

*Public search and record query shortcuts are ergonomic Signal operations.*
Generated `Input::PublicTextSearch(SearchText)` is the ordinary agent-facing
lookup path: it searches active public records by description text and referent
text, ranks likely matches, tolerates unregistered words as search terms, and
returns capped `RecordsObserved` results directly rather than a stash handle.
Generated `Input::PublicRecords(RecordSelection)` and
`Input::PrivateRecords(RecordSelection)` expose privacy-scoped structured reads
without making callers spell the full `Query` object every time. Nexus lowers
those record-selection roots into schema-declared `CommandSemaRead(Observe(Query))`:
`PublicRecords` uses exact-`Zero` privacy, and `PrivateRecords` uses non-zero
privacy (`AtLeast Minimum`). Both project to ordinary observation certainty
(`AtLeastCertainty Minimum`) and unconstrained importance, so zero-certainty removal
candidates stay out of normal query surfaces. SEMA still owns the canonical
`Query` predicate and durable read behavior.

*Certainty and importance are separate axes.* `Entry` stores `Certainty` and
`Importance` separately. Certainty names confidence/currentness: `Zero` nominates a
record for removal while direct `Lookup` remains possible. Importance names
importance/repetition and drives retrieval order/filtering. Importance must not be
overloaded onto certainty.

*Guardian admission requires affirmative guidance.* Per Spirit record `nr7h`,
intent captures should state the positive shape to follow: what the practice is,
what the component does, what spelling/name/contract is canonical, or what
boundary holds. When a proposed record's operative guidance is framed primarily
as an exclusion, prohibition, forbidden wording list, or definition by negation,
the guardian rejects it with `NegativeGuideline` so the submitting agent can
re-plead the affirmative rule.

*Guardian approval is a high bar across the whole operation.* Per Spirit record
`7xnx`, the guardian is a strict admission-control gate for every working-signal
submission. It judges the submitted operation, testimony, reasoning, magnitudes,
domain, privacy, collision with active intent, affirmative framing, and
operation fit as one case. A fresh `Record` is admitted only when it is genuinely
new; if the psyche statement refines, narrows, corrects, or explains an
identifiable existing record, the guardian rejects the fresh record and remands
the agent to `Clarify`, `Supersede`, `ChangeRecord`, `ResolveClarification`,
`Retire`, or `Remove` as appropriate.

*Settled referent registrations do not need guardian judgment.* Per Spirit
record `bwxn`, when a referent registration request names a referent and aliases
that already resolve to one registered referent, the registry itself settles the
case. Spirit returns the existing canonical referent receipt without calling the
referent guardian and without mutating the store. Adding a new alias or new
referent is still a real registry change and remains guardian-gated when the
guardian feature is active.

*Intent submissions imply missing referent registration.* Entry-bearing writes
(`Record`, `Propose`, `ChangeRecord`, and `Supersede` replacement entries) treat
each listed `Referent` as an embedded `ReferentRegistration` request with no
aliases and the write's own `Justification`. The implied registration runs
through the same settled-state and referent-guardian path as explicit
`RegisterReferent`; a rejected implied registration blocks the entry write.
This is not hidden parser logic: Nexus exposes the stages as generated
`RecordWithImpliedReferents`, `ProposeWithImpliedReferents`,
`SupersedeWithImpliedReferents`, `ChangeRecordWithImpliedReferents`, and the
matching `*ReferentsSettled` results before the normal guard/write operation.

*Nexus is the recursive runner payload keeper and the internal feature catalog.*
Signal triage produces a generated `nexus::Nexus<nexus::Work>` envelope;
`triad-runtime::Runner` owns the continuation budget and repeated dispatch.
Hand-written `Nexus` implements one decision step, SEMA write/read hooks, and
the effect hook. Per intent record `gvaz`, computations, result filters,
conditional writes, and similar internal engine features must appear as Nexus
schema verbs/objects before Rust implements them, so the daemon's feature
surface stays visible in `schema/nexus.schema` instead of hidden in ad-hoc
implementation code.
Per intent record `k4d9`, that schema visibility does not imply broad
crate-root export: the generated plane modules are the schema API surface.
Generated ordinary Signal nouns stay public through
`spirit::schema::signal` as a re-export of the `signal-spirit` contract; Nexus
and SEMA nouns stay public through `spirit::schema::{nexus,sema}`. The daemon
crate root exports only hand-written runtime composition objects such as
`Engine`, `Nexus`, `Store`, `Daemon`, `SignalTransport`, and trace/runtime
errors.

*Reusable triad role names are shared traits, not component enum names.*
Spirit's generated roots implement `triad-runtime` role traits such as
`NexusWork`, `NexusAction`, `SemaWriteInput`, and `SemaReadInput`. A reusable
role name can then be spoken by the runner without pretending Spirit's
component-specific variants are the universal shape for every component.

*The daemon listener shell is shared runtime, not Spirit boilerplate.* The
generated `DaemonBinder` constructs `GeneratedDaemonRuntime<SpiritDaemon>`
around `Engine`, then hands it to `triad_runtime::AsyncMultiListenerDaemon`
with a working listener and a required meta listener. The shared runtime
creates parent directories, removes stale socket files, binds Unix listeners,
captures accepted-connection context, starts and stops the component runtime,
keeps listeners alive after per-request transport errors, and owns the emitted
subscription registry / retained writer plumbing. Spirit owns only binary
configuration decoding, engine construction, one working-input hook, the
owner-only meta request hook, and stream filter/event policy.

*SEMA is durable.* `Store` maps generated SEMA roots onto `sema-engine` keyed-record operations over a `.sema` file. Record identifiers are production-compatible short/base36 string keys, not sequential numeric counters; migration imports production identifiers unchanged, and fresh records mint unused short keys. Each `Record` asserts a keyed `StoredRecord`, each `ChangeCertainty` and `ChangeRecord` mutates the same key, each `Remove` retracts that key, and `Observe`/`Count` read through sema-engine query plans while `Lookup` bypasses filters by exact key. SEMA keeps `DatabaseMarker` as internal/database introspection; the public Signal surface exposes it only through `Marker`.

*The store is a fold of its versioned log.* Per Spirit record `iir4` (Decision,
High certainty): [The versioned operation log is the authoritative source of
truth for component Sema state, and the redb store becomes a rebuildable
materialized view folded from the log. This kernel inversion is chosen for the
first version-control implementation rather than deferred.] The stored record
families (`StoredRecord`, `StoredReferent`, `Migration`) are declared in
`schema/sema.schema` as `RecordsFamily`/`ReferentsFamily`/`MigrationsFamily`;
the generated module carries the per-family content-hash identities, the closed
`RecordFamily` sum, the table descriptors, and the `versioning_policy()`.
`Store::open` opts in through that generated surface only — no hand-built
policy, family name, or hash exists in spirit — so every durable write lands a
replayable versioned log entry, checkpoints with payload restore through the
engine-owned import session, and the daemon-level query surface of a restored
store is identical to the original.

*Migration is a logged fold; the copy-everything binaries retired.* Per Spirit
record `t0tu` (Decision, High certainty): [Spirit's next schema bump is the
pilot for migration as a logged fold: replay the previous store's versioned
operation log through the version From-chain into a store at the next schema,
recording a typed migration entry. The copy-everything migration binaries
retire; migration becomes a fold the version-control system records rather
than an unlogged database rewrite.] Schema version 9 is that pilot's bootstrap
case: versions 7 and 8 carry no versioned log, so `StoreMigration` reads
them with `sema-engine-previous` (the generation that wrote them), converts
through the historical `From`-chain, and writes every record, referent, and
the typed `Migration` marker into the fresh version-9 store through the
ordinary logged choke points — the migrated store's log is the first complete
history a spirit store has carried. No pre-version-7 store exists anywhere
(psyche decision), so a probed version below 7 is rejected outright rather
than folded forward. `spirit-migrate-production` and
`spirit-upgrade-store` retired; `spirit-migrate-store` is the one migration
entry point. From version 9 onward the previous store's LOG is the fold input.

*The daemon's single argument is a path to a binary rkyv `SpiritDaemonConfiguration` object from the `signal-spirit` contract.* The packaged `spirit-write-configuration` text-edge helper may create that file from a typed NOTA request for launch/deploy tooling, but the daemon startup path only decodes binary state. The daemon wraps the decoded contract value in its local `Configuration` runtime object so it can implement `BindingSurface` without moving runtime behavior into the contract crate.

*Criome authorization mode is startup policy.* The binary configuration carries
`AuthorizationMode`. `Gating` is the fail-closed production posture: the local
criome verdict releases or holds mirror fan-out. `Observing` is the
trace-scaffold posture per Spirit records `2st7` and `ef6i`: spirit sends the
same criome authorization request, waits long enough to observe criome's
returned verdict, emits trace evidence for the authorization-return point, and
still proceeds without applying that verdict as a blocking gate in the first
production/demo posture. This lets mentci and introspection observe the request
stream while local writes continue. That emitted request stream is
observability/tracing substrate for
future mentci-mediated acceptance gating, not a separate non-trace production
watch channel.

*The daemon configuration carries the meta slot.* Per Spirit record `pb1g`
(High certainty), every component needs a meta slot because configuration and
policy authority must not live on the ordinary working signal. Spirit's
`Configuration` stores `meta_socket_path` as an `Option` because the shared
`BindingSurface` trait uses that shape, but the generated Spirit daemon
rejects `None` with `MissingMetaSocket` before serving either socket. The
meta signal wire vocabulary is imported from `meta-signal-spirit`, parallel to
the ordinary working signal vocabulary imported from `signal-spirit`.

*Trace is optional runtime instrumentation.* The `testing-trace` surface observes Signal/Nexus/SEMA calls through generated trait hooks without affecting production binary behavior. Trace events carry schema-generated typed `ObjectName`, not free strings. Spirit owns the typed `TraceEvent` over plane-local object names; `triad-runtime` owns the reusable log, frame mechanics, and client-side collection.

Load-bearing proof: the real process boundary is tested, not only in-memory function calls. Trace events cross the Signal admission, `SignalEngine`, `NexusEngine`, and `SemaEngine` boundary, proving actor/interface use instead of source-string presence.
