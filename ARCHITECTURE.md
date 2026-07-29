# ARCHITECTURE — spirit

## Purpose

`spirit` is the running proof that generated schema can create an interface
used by a real CLI and daemon pair.

**Current implementation: wired legacy toolchain, not the approved
architecture.** This repo currently builds through the schema/schema-language/
schema-rust pipeline described below. That pipeline is dead under its old
name: the language was renamed Ethos on 2026-07-27, and legacy schema,
schema-language, and schema-rust die under their old names (S1R entry 7).
Spirit's port onto Ethos-based generation has not landed; it is in progress
and is blocked at bead `protos-engine-po1.10.11`. The spirit-port acceptance
test requires a working system against an isolated migrated copy of
production data, zero build or runtime dependency on schema-rust, and no
compatibility adapters (SSR entry 8, psyche-confirmed). Until the port lands,
do not copy this repo's schema/schema-rust build pipeline as the pattern for
new components.

The repo is intentionally one daemon crate, but it is not an all-in-one
schema shape: the ordinary public signal/domain contract is generated in the
`signal-spirit` contract crate, the owner-only meta-policy signal contract is
generated in the `meta-signal-spirit` contract crate, and Spirit generates only
daemon-local Nexus, SEMA, and daemon runtime modules through the shared driver.
It consumes those contracts and daemon-local modules through `triad-runtime`
plus `sema-engine`. The plane/runtime shape — split ordinary/meta contract
crates, daemon-local Nexus/SEMA generation — is the pattern future component
daemon repos should reproduce once it is carried onto Ethos-based generation;
the schema/schema-rust generation step itself is superseded.

## Intent model

The unit of intent is the **domain**: a specific grounded subject grouped under
broad areas (the older "category" and "topic" terms are retired). `Domain` is a
closed enum with a broad category-theory starting set, represented as grouped
schema enums by area (for example `Home` carrying `Housing`, `Maintenance`, and
siblings) rather than one flat enum or free text. The domain vocabulary is a
variable-depth tree where depth tracks intent density: dense branches like
`Software` nest a third tier of fine subjects while sparse domains like
`Hardware` stay one or two tiers — variable depth is the rule, not an exception.
Domain names must be self-explanatory and carry meaning in the variant name
itself; there is no separate gloss or description layer, and an unclear name is
renamed rather than annotated.

The hierarchy is structural — tree nesting plus scope-prefix matching, not a
directional relation. A domain nested under another is subsumed by it, and
cross-tree relations are symmetric equivalence only (synonyms that retrieve
together both ways); directional subsumption is dropped because nesting already
carries all the hierarchy. A `DomainScope` is therefore a typed nested prefix of
the `Domain` enum, written in the same nested-paren form as a domain value (for
example `Technology` containing `Software` containing `Quality`), not a flat
vector of free string segments: each segment is a real enum variant, so an
invalid or misspelled segment is rejected at parse time.

The canonical stored record stays one uniform shape — no per-kind storage
variants. A creation shorthand or a per-kind view is a presentation/ergonomics
layer over the uniform record, not a second stored shape. The record carries
`Certainty` and `Importance` as two orthogonal `Magnitude` axes, plus a privacy
field. Privacy is a directional `Magnitude` on its own axis: `Zero` means
open/public and higher magnitudes narrow the intended audience, reusing the
existing `Magnitude` vocabulary (with `AtMost`/`AtLeast` filters paired to the
certainty selectors) rather than introducing a separate audience-register enum.
The earlier framing of privacy as an `Optional` field — `None` for public,
`(Some Magnitude)` for elevated, with the `None` token always written explicitly
since every NOTA positional record carries every field — is the same intent
expressed before the axis was unified onto `Magnitude`; the current model is the
privacy-`Magnitude` axis with `Zero` as the default public level. (Privacy
remains nominal, not a security boundary — see Known limits.)

Spirit operations support a **variant ladder**: short forms with summary defaults
for common operations, complex forms with full metadata for precise control. Both
coexist as distinct wire operation roots; the complex root is canonical and the
short root expands to a default of it. A `RecordQuery` composes filter dimensions
— topic selection (multi-topic, partial any-of or full all-of, plus
topic-catalog queries), optional `Kind`, a certainty selection that can target
removal candidates by magnitude, and a first-class recency selection (`Any`,
newest N, since an identifier or timestamp, or a window range) — alongside
identifier lookup, public-only/private-only shortcut verbs, and an explicit
privacy-query subtype exposing at-most/equal/at-least selectors while ordinary
queries stay `Zero`-only. The verbal depth-scope vocabulary is settled as
`Shallow`/`Recent`/`Deep`/`VeryDeep` with target counts 5/15/30/100 (bare
`Recent` adopting the explicit 15), the new variants appended at the end of
`RecordedTimeSelection` to preserve rkyv discriminant stability. A separate
small-record type carries only the load-bearing fields — identifier, topics,
kind, description summary, magnitude, daemon-stamped time — distinct from the full
record's metadata; short-form reads and `CollectRemovalCandidates` emit it and
archiving consumes it, and its outcomes report as typed variants
(archive-created, archive-appended, backup-archive-used), never string messages.
The timestamp is daemon-stamped throughout.

## Layers

```text
signal-spirit/schema/{domain,signal}.schema
  -> signal-spirit/src/schema/{domain,signal}.rs
  -> Spirit ordinary dependency schema
meta-signal-spirit/schema/meta-signal.schema
  -> meta-signal-spirit/src/schema/meta_signal.rs
  -> Spirit meta dependency schema
schema/{nexus,sema}.schema
  -> build.rs
  -> schema_rust::build::GenerationPlan with dependency schema + daemon-local targets
  -> schema_rust::build::GenerationDriver
  -> schema::SchemaSource typed source objects inside the shared driver
  -> rkyv-serializable schema-in-Rust values checked by the shared driver
  -> schema-rust::RustEmitter with opt-in NOTA surface inside the driver
  -> checked-in generated daemon-local modules at src/schema/{nexus,sema,daemon}.rs
  -> engine composer + nexus mail keeper + sema-engine backed store + transport
```

Each generated module has one binary floor and one optional text surface.
`rkyv::Archive` / `Serialize` / `Deserialize` are always emitted because every
component speaks binary frames. `nota::NotaDecode` / `NotaEncode`, root
`FromStr`, root `Display`, and `to_nota` helpers are emitted behind the
`nota-text` feature. That lets the CLI crate target parse and print NOTA while
the daemon target compiles without the NOTA decoder linked into its runtime
surface. `tests/dependency_surface.rs` is the executable guard: the normal
dependency tree with `--no-default-features` must contain no `nota`, while
the `nota-text` tree must contain it.

There is no daemon-side printline anywhere, by rule. The daemon observes only
through its own typed schema-defined trace interface (and any future logging
surface); stderr printline statements as a fallback for in-band trace errors are
forbidden, because they would break the typed-data-strings-only-at-display
discipline. Trace code is optional at compile time so a lean build carries none
of it, and the build configuration is itself a NOTA struct under the same
single-NOTA-argument rule as the daemon and CLI, with a testing-build field
switching production and testing modes.

`testing-trace` is a second explicit surface, separate from the normal
runtime. Normal Nix packages build a lean binary daemon plus NOTA CLI adapter.
Trace packages build the same pair with `testing-trace`; the daemon can emit
rkyv `TraceEvent` frames to a configured trace socket, and the CLI can listen
on that socket and render decoded trace events after the ordinary Signal
reply. Spirit owns the typed aggregate `TraceEvent` over plane-local generated
object names and actor hook emission. `triad-runtime` owns the reusable trace
log, length-prefixed binary frame, Unix trace socket listener, and generic
client-side trace collector. A trace event carries an `ObjectName`, and that
object name is supplied by the generated trait wrapper
(`SignalObjectName::Triaged`, `NexusObjectName::Entered`,
`SemaObjectName::WriteApplied`, `SemaObjectName::ReadObserved`, and siblings),
not a free string or cloned payload snapshot. The CLI renders the decoded
`TraceEvent` through its typed NOTA surface via the shared typed trace client,
producing one object such as `(Sema WriteApplied)` rather than a one-field
wrapper around it. The trace path is a runtime proof surface, not deployment grep: the
process-boundary test starts a real daemon, sends real CLI requests, decodes
each displayed NOTA trace line back into `TraceEvent`, and asserts the
Signal/Nexus/SEMA event sequence that returns over the trace socket.

The current plane schemas intentionally keep braces strict as NOTA key-value
maps. The Signal namespace contains pairs such as `Referent String`,
`RecordSet (Vec ObservedRecord)`, and
`Entry { Domains * Kind * Description * Certainty * Importance * Privacy * Referents * }`; it does not
contain declarations that repeat their own name inside the value.
Inside a struct map, `Domains *` derives the `domains` field from the existing
`Domains` type, and explicit bindings such as `kind (Optional Kind)` stay only
where the field name differs from the referenced type. Bare reference
declarations (`Referent String`, `RecordSet (Vec ObservedRecord)`, `Record RecordRequest`) become
exported aliases in the typed schema value and generated Rust, so enum variants
carry direct payloads instead of wrapper structs. Explicit brace-body singleton
declarations are the newtype form.

Enum bodies keep vector homogeneity by listing exported object names at root
positions and by using one signature object per namespace enum variant.
Namespace bindings such as `Record RecordRequest`, `RecordAccepted RecordIdentifier`, and
`SignalArrived Input` define the payload shape for data-carrying root objects;
names without payload bindings are unit variants. Inside namespace enums,
same-named payload variants use the compact `(Record)` form, while explicit
`(Variant Payload)` is reserved for different names. The vector does not
contain pseudo key-value pairs.

The three runtime centers are concrete objects: `SignalAdmission` (admission),
`Nexus` (mail keeper + translator, owns the store + ledger), and `Store` (the
durable SEMA plane over `sema-engine`). `Engine` composes them and owns no
SEMA state of its own.

The generated engine traits carry lifecycle hooks. `Engine::start` calls
`NexusEngine::on_start`, which starts its owned SEMA store through
`SemaEngine::on_start`, then starts Signal through `SignalEngine::on_start`.
`Engine::stop` reverses the boundary: Signal stops first, then Nexus, then the
owned SEMA store. The hooks are default no-ops in normal builds; trace builds
override them and emit typed generated object names. This is the current
addressable lifecycle surface; full actor mailbox and runtime-control
machinery remain future work.

`DaemonCommand` is the current programmatic startup noun. The daemon binary
`main` only creates `DaemonCommand::from_environment()` and runs it; the
command loads the single binary configuration path and constructs `Daemon`.
`Daemon` asks the generated `DaemonBinder` for an
`AsyncMultiListenerDaemon<GeneratedDaemonRuntime<SpiritDaemon>>`. The generated
binder opens the component `Engine`, wraps it in async task-backed runtime state,
and passes a working listener plus a required owner-only meta listener to
`triad-runtime`. The shared runtime prepares each Unix socket, binds the
listeners, captures accepted connection context, starts the runtime, serves
accepted streams by listener tier, logs request-level errors without stopping
the listeners, and stops the runtime when the accept loop exits. Spirit owns
only engine construction and the component hooks behind the generated trait.

This is the live startup-runner slice: startup and listener behavior belongs to
shared runtime nouns, while domain decisions belong to generated engine trait
implementations.

## Borrowed prototype lessons

- From `design-nota-from-schema`: make the recursion floor explicit and keep
  generated source as source, not hidden macro behavior.
- From operator `schema-rust`: schema emits Rust code first; Rust macros
  are a later ergonomic surface.
- From designer Spirit POC: keep actor/runtime boundaries visible, but avoid
  the retracted `EffectTable`/`FanOutTargets` authored-schema path.
- From the nspawn pipeline prototypes: prove the real process boundary, not
  only in-memory function calls.

## Runtime triad

The daemon is shaped as the Signal / Nexus / SEMA runtime triad.

### Signal

The CLI:

1. reads one NOTA argument;
2. parses it into generated `Input`;
3. asks generated `Input` to frame itself as short-header + rkyv archive
   bytes;
4. sends it over a Unix socket;
5. decodes generated `Output`;
6. prints NOTA.

The CLI parse path is generated-first. `State` uses the generated canonical
shape `(State [text])`; the daemon never receives NOTA text at all.

The daemon:

1. starts from `DaemonCommand`, which accepts exactly one argument: a path to a
   binary rkyv `signal_spirit::SpiritDaemonConfiguration` object, decoded into
   the daemon-local `Configuration` wrapper;
2. constructs `GeneratedDaemonRuntime<SpiritDaemon>`, opening the configured
   `.sema` database path through `Store`;
3. hands that runtime plus the configured working and required owner-only meta
   socket paths to `triad_runtime::AsyncMultiListenerDaemon`;
4. starts the generated engine lifecycle through the async task-backed runtime;
5. reads a length-prefixed binary frame from each accepted stream;
6. asks generated `Input` to triage by short header and decode itself (the
   working socket decodes signal `Input`; the meta socket decodes meta-signal
   `Input`);
7. dispatches through `Engine`;
8. asks generated `Output` to frame itself as binary rkyv;
9. writes it back.

The daemon does not parse NOTA at startup and does not need `nota` for
its binary-only build. `spirit-write-configuration` is the packaged text-edge
helper for launch/deploy tooling: it accepts a typed
`ConfigurationWriteRequest` NOTA record and writes the binary configuration
archive before `spirit-daemon` starts. Production configuration should later
become another typed binary signal surface differentiated by the root message
enumerator, not a daemon NOTA side channel.

The binary configuration carries `AuthorizationMode`: `Gating` blocks a
guarded operation on the criome operation authorizer's verdict, and
`Observing` emits the authorization request without blocking on it. It governs
the guardian OPERATION authorizer only; head fan-out no longer reads it. Head
authorization is governed by the spirit-side `CriomeAuthorization` option
instead (see the mirroring section below). The earlier 1-of-1 local head
authorization — a co-resident criome daemon authorizing the content-addressed
head with a single local signature — is DELETED, not kept as a mode;
criome-cluster authorization is LIVE as the everywhere-gate on the intake
path (see the mirroring section below). Production alignment and orchestration rely on Spirit and do not
depend on Mind; Mind stays future/non-production until the psyche marks it
production-ready. It also carries the daemon's meta slot as `meta_socket_path`.
The field is optional in the data type because the shared
`BindingSurface` trait asks for `Option<&Path>`, but Spirit treats absence
as a startup error (`MissingMetaSocket`) and binds no working listener without
it. When the path is present, the daemon binds a SECOND listener on it — the
owner-only meta-signal surface — distinct from the ordinary working socket, so
policy and configuration authority have a component-owned home apart from the
peer-callable working signal. The meta contract is imported from the
`meta-signal-spirit` contract crate, generated from
`meta-signal-spirit/schema/meta-signal.schema`: it carries the `Configure`,
`Import`, and `CollectRemovalCandidates` `Input` roots, the
`Configured`/`Imported`/`RemovalCandidatesCollected`/`Rejected` `Output` roots,
their records, and the rkyv derives — no Nexus/SEMA planes and no engine
traits. The first owner-only operation is
`Configure(ConfigureRequest { ArchiveDatabaseTarget })`, where
`ArchiveDatabaseTarget` is the ported `[Default | Path(ArchivePath)]` enum. It
sets WHERE the SEPARATE archive database lives — the destination the owner-only
`CollectRemovalCandidates` meta operation archives into. The
archive database is a distinct `*.sema` file from the live intent log;
`Configure` does NOT open, move, or touch the live database. The daemon applies
it through `Engine::configure` (an owner-config effect under the same
single-flight Nexus mutex the working path uses, NOT a SEMA log write), which
stores the target on the SEMA `Store` (`Store::set_archive_target`, a field +
accessor) and replies with the now-active target plus the live database marker.
`ConfigureRequest` also carries the optional mirror, criome-gate, and
guardian-prompt targets the same operation applies in one owner round-trip;
each is runtime policy applied to the live engine, not a SEMA log write, and
echoed in the `Configured` receipt. The criome-gate target arms BOTH the
cluster acceptance gate (`CriomeAuthorization::Enabled` with the cluster
authorizer at the given socket) and, under the guardian, the operation-level
authorizer at the same socket; `Default` or absence disarms both. Enabling
the gate also resolves any crash-parked staged group (§3.8 recovery) and
fires one residue-reconcile ship mail.

Mirroring is not a separate component: it is how Spirit operates over criome and
the sema mirror. Spirit knows NOTHING about quorums (primary-6kz1): it constructs
no proposal, holds no round state, and names no threshold. Whether acceptance
itself is subject to criome authorization is the spirit-side
`CriomeAuthorization` option, a closed typed enum on the `CriomeGate` seam
(its own `criome-gate` feature — acceptance gating is never compiled out with
shipping). `Disabled` is the operative default: Spirit runs fully local —
heads advance freely, nothing propagates. `Enabled` is THE EVERYWHERE-GATE
(the 2026-07-07 psyche correction): the quorum gates acceptance everywhere,
including locally. A head-advancing working input runs stage → authorize →
materialize: the nexus pipeline builds its operation group over the
sema-engine staging session (reads against committed state plus the in-group
overlay, nothing committed), the group durably parks with its PROSPECTIVE
head digest, the connection task drains the criome authorization session
outside the engine mailbox (reads keep flowing; head advances serialize
first-in first-out through the shared advance gate), and only the pushed
cluster grant materializes the group — one atomic transaction — and releases
the held accepted reply. Every other terminal verdict (`Denied`, `Expired`,
`Unavailable`, `Unreachable`, or a gate machinery fault) refuses the
OPERATION to the caller as `Output::AdvanceRefused` with its closed reason,
and the staged group is discarded: nothing is recorded anywhere, fail-closed,
no default-open branch. The durable staging slot exists for crash windows
only (§3.8): an occupied slot at daemon start refuses head advances until an
owner `Configure` re-enables the gate and recovery re-asks with the parked
digest — a recovery grant MUST materialize (the cluster already accepted the
operation), a refusal discards. Shipping is pure DISTRIBUTION of accepted
state: on each materialization the ship drain receives "head advanced" mail
and ships the unshipped suffix, re-fetching the standing committed head's
immediate re-grant at ship time (no new cluster round); the ONE genuinely new
drain round is disabled-era residue, covered transitively by a single batch
grant. There is NO spirit-side apply ingress: Spirit
does not accept a foreign authorized-record apply on its working socket. The
`signal-spirit` contract retains the working-tier `ApplyAuthorizedRecord` variant
for wire compatibility, but the daemon answers it fail-closed with
`ApplyRefusalReason::AuthorizationUnavailable` — no criome round-trip, no store
write. Spirit's only guardian-bypassing write path is the owner-only meta
`Import` below.

The second owner-only operation is `Import(ImportRequest { [ImportedRecord] })`,
each `ImportedRecord { RecordIdentifier, Entry }`: it writes pre-vetted records
straight to the live SEMA store with their given identifiers, BYPASSING the
guardian admission pipeline (`Engine::import` over `Store::import_record`). This
is the privileged restore/migration path — corpus rebuild, disaster recovery,
machine moves. Guardian-bypassing writes exist ONLY here on the owner-only meta
socket; the working signal stays fully gated. It aborts to `Rejected` on the
first store error so a partial import is loud. The `meta-spirit` CLI is the
owner-only client for these operations — the privileged sibling of `spirit`.

The third owner-only operation is
`CollectRemovalCandidates(CollectRemovalCandidatesRequest { RemovalCandidateCollection })`:
it archives every record matching the supplied query into the SEPARATE archive
database at the configured target, then physically retracts it from the live
log, replying `RemovalCandidatesCollected` with the archived / removed / skipped
triple. It runs with NO guardian, mirroring `Import` and `Configure`
(`Engine::collect_removal_candidates` over the unchanged
`Store::collect_removal_candidates` archive-then-retract primitive). This is the
ONLY physical-deletion path in the component: there is no working-socket delete
operation. `&mut self` on the meta op gives it the same single-flight
serialisation against every working write that `Import`/`Configure` rely on.

The authority split is load-bearing: physical deletion is owner-only. The OWNER
configures WHERE archives go (meta `Configure`) and issues the deletion (meta
`CollectRemovalCandidates`); no peer can physically remove a record. The earlier
`set_archive_target` re-opened the LIVE `SemaDatabase` at the new path — that was
the bug; the live log is now never disturbed by a reconfigure.

The daemon binds both sockets through
`triad_runtime::AsyncMultiListenerDaemon`, tagging each accepted connection
with `ListenerTier` (`Working` / `Meta`) so `GeneratedDaemonRuntime` decodes
the correct wire contract for the arriving socket. Owner-only authority rests
on the meta socket file mode (`0o600`, applied via `SocketMode`) and listener
tier; accepted connection peer credentials are available in
`ConnectionContext`, but Spirit's current origin route is internal and
monotonic rather than uid-derived. The missing-meta case is a startup error,
not a single-socket compatibility mode. The meta wire codec (an 8-byte short-header frame
byte-identical to the signal plane's) lives on the generated `Input`/`Output`
nouns in `src/meta_transport.rs`, because the `WireContract` emission target in
the current `schema-rust` pin emits the per-root `short_header` constants
but not the `encode_signal_frame`/`decode_signal_frame` frame codec.

`SpiritDaemonConfiguration` may also carry a trace socket path. That field is
still binary rkyv configuration; it does not add a NOTA startup path to the
daemon. Only a daemon compiled with `testing-trace` uses it. The daemon writes
trace frames as binary objects, and only the trace-enabled CLI decodes them into
text.

The binary socket also rejects text structurally. `tests/socket_negative.rs`
feeds length-prefixed NOTA bytes and arbitrary bytes through `SignalTransport`
and directly through the generated `Input::decode_signal_frame` method. Those
tests prove that NOTA is accepted only by the CLI text surface, never as daemon
wire input.

The hand-written transport module owns only the component-specific bridge
between generated signal frames and `triad-runtime::LengthPrefixedCodec`. It
does not own route enums, short-header matching, or rkyv archive encode/decode.

`SubscribeIntent(Query)` is the first streaming Signal operation. The authored
signal schema marks it as opening `IntentEventStream`; generated Rust therefore
exposes the stream-capable `Frame`/`FrameBody` aliases and
`IntentEvent::into_subscription_frame`. The CLI still sends exactly one NOTA
input. For ordinary inputs it prints one output and exits; for
`SubscribeIntent` it prints `SubscriptionStarted` and keeps reading
length-prefixed `signal-frame` subscription-event frames, rendering each as
generated `Output::Event(IntentEvent)` at the human edge.

`Version` is the bare NOTA Signal input for asking the running component what
package version it is. It follows the same one-argument CLI rule:
`spirit Version` enters the generated Signal frame path and Nexus replies with
`VersionReported(VersionReport { version_text, database_marker })`.

The daemon handles a subscribe request through generated async task-backed stream
plumbing in `src/schema/daemon.rs`: it writes the ordinary
`SubscriptionStarted` reply, stores the accepted connection's Tokio writer half
under the Nexus-minted `SubscriptionToken`, registers the filter in a
`SubscriptionRegistry`, and returns to the accept loop. The generated
`EmittedSubscriptions` object owns the live registry, event publisher, and
retained writers; Spirit only supplies the token extraction, filter matching,
event construction, and event short-header policy hooks.

### Nexus

`SignalAdmission::admit` is the Signal validation/admission point. It mints the
origin route, issues the Spirit runtime `MessageSent` event, validates generated
`Input`, and produces `SignalAccepted`. `SignalAccepted::process_with` is the
flat composition point: it emits the sent event, asks `SignalAdmission::triage`
for a generated `nexus::Nexus<nexus::Work>`, runs `NexusEngine::execute`, emits
the Spirit runtime `MessageProcessed<Output>`, and asks
`SignalAdmission::reply` for the Signal output.

`Nexus` is a real runtime object over schema-emitted roots. It owns the durable
SEMA `Store` handle, the `MailLedger`, the `StashTable`, and the
`ClassificationPolicy`, and it implements the generated mutable `NexusEngine`
trait. The schema is also the internal feature catalog: per intent record
`gvaz`, any computation, result filter, conditional write, or similar engine
feature is added first as a Nexus verb/object in `schema/nexus.schema`, then
implemented by the hand-written runtime object.
Per intent record `k4d9`, internal feature nouns being schema-visible does not
mean they are flattened into the daemon crate root. The generated plane module
(`spirit::schema::nexus`) is the public internal schema API; the crate root is
only an ergonomic barrel and should not become a mixed namespace for every
internal noun unless a real external consumer needs a deliberate top-level
export.

```text
signal::Signal<Input>
  -> SignalEngine::triage
  -> nexus::Nexus<NexusWork>
  -> NexusEngine::execute(&mut Nexus, ...)
  -> nexus::Nexus<NexusAction>
  -> SignalEngine::reply
  -> signal::Signal<Output>
```

The `&mut Nexus` borrow on `NexusEngine::execute` is the single-flight guard:
Rust prevents two mutable executions on the same Nexus at the same time.
Per Spirit record 1339, the one working path is the schema-plane trait path:
Signal emits a generated Nexus envelope, Nexus executes it, and SEMA is reached
only through generated split write/read roots.

Generated `NexusEngine::execute` owns the central runner loop through
`triad-runtime::Runner`; the hand-written `Nexus` object owns one decision
step and the component hooks. `NexusWork` is the fact stream Nexus decides
from: Signal arrivals, SEMA completions, effect completions, and recursive
work. `NexusAction` is the command stream Nexus emits next:
`CommandSemaWrite`, `CommandSemaRead`, `CommandEffect`, `Continue`, or
`ReplyToSignal`. The generated runner adapter projects those actions into
`triad_runtime::NextStep`, calls the `NexusEngine` storage/effect hooks, and
stops with a typed budget-exhausted reply if recursion does not reach Signal.
The processed ledger event is generated `MessageProcessed<Output>`, so the
same `OriginRoute` is carried from Signal admission through Nexus, SEMA,
effects, and back to the Signal reply.

Nexus is the home for non-default decision algorithms. Frecency ranking,
co-occurrence decisions, semantic similarity, future topic-discovery logic,
computed result filters, and conditional writes should extend `NexusEngine`
behavior through generated Nexus work/action/effect nouns. SEMA owns the
durable indexes and tables those algorithms read; Signal remains the
communication boundary. One intended direction is intelligent topic retrieval
that emphasizes recent intent by default while adapting historical depth to
topic frequency — high-churn topics staying near the edge, quiet topics reaching
farther back for a comparable useful result set — growing search beyond simple
filters toward weighted keyword or recency scoring expressed in the Nexus
language. This is an exploratory direction, not a settled immediate requirement.

Nexus write commands are feature-specific. `CommandSemaWrite` is not a raw
pass-through alias for the whole SEMA write enum; it is a Nexus-plane enum with
visible `Record`, `ChangeCertainty`, and `ChangeRecord` objects. The
generated runner still treats it as the fixed `SemaWrite` outcome, but the
component schema keeps each internal write feature readable in
`schema/nexus.schema`.

`State` classification follows that rule. Signal admits the raw statement, but
does not classify it and does not open storage. Nexus first emits
`CommandEffect(ClassifyState(...))`, proving the classification feature is
declared in `schema/nexus.schema` instead of hidden inside a direct write
branch. The effect implementation applies the fallback classification policy
(`unclassified`, `Clarification`, `Minimum`, `Zero`) and returns
`EffectCompleted(StateClassified(...))`; the next Nexus decision emits
`CommandSemaWrite(Record(...))`. SEMA then persists the generated `Entry`
through the same write root used by ordinary `Record` input. This ports one
deployed `persona-spirit` behavior without reviving the old actor tree in the
daemon.

`SubscribeIntent` follows the same Nexus visibility rule. Signal admits the
query, Nexus emits `CommandEffect(OpenIntentSubscription(Query))`, the effect
uses `triad-runtime::SubscriptionTokenIssuer` to mint a token, and Nexus
returns `IntentSubscriptionOpened(IntentSubscription)` before replying to
Signal as `Output::SubscriptionStarted`. The daemon does not mint hidden
subscription identity; it only attaches the already-declared and already-minted
token to a live socket writer. Successful `Record` writes are projected back
through `Engine::intent_recorded_event`/`Nexus::intent_recorded_event`, so the
daemon never opens SEMA directly to publish events.

`ChangeCertainty` and `ChangeRecord` are production mutation parity slices.
Signal admits the generated `CertaintyChange` or `RecordChange` payload. Nexus
emits the schema-declared `CommandSemaWrite(ChangeCertainty(...))` or
`CommandSemaWrite(ChangeRecord(...))`. SEMA looks up the keyed record and
writes back through a keyed mutation; certainty changes mutate only the stored
entry's `Magnitude` through the `Certainty` alias, while record changes replace
the full stored `Entry` under the same `RecordIdentifier`. The reply is
`CertaintyChangeReceipt` or `RecordChangeReceipt` with a new database marker.
`Zero` certainty is the soft-removal candidate state. Ordinary observation uses
`AtLeastCertainty Minimum`; explicit review uses `ExactCertainty Zero`; direct
`Lookup` remains available by record identifier.

`PublicTextSearch`, `PublicRecords`, and `PrivateRecords` are ergonomic read
shortcuts. `PublicTextSearch(SearchText)` is the common agent-facing search
path: Signal admits one text payload, Nexus projects it to the schema-declared
SEMA `PublicTextSearch(SearchText)` read, and SEMA searches active public
records by description text and referent text, ranks likely matches, and
returns capped `RecordsObserved` results directly. It intentionally does not
canonicalize referents first, so unknown words are search terms rather than
errors. `PublicRecords` and `PrivateRecords` admit a generated
`RecordSelection` payload — topic match plus optional kind, without a privacy
field. Nexus projects those to canonical SEMA `Observe(Query)`: public means
exact `Zero` privacy, private means `AtLeast Minimum` privacy. This keeps
friendly working-signal verbs visible while preserving the full `Query`
predicate for structured/exhaustive reads.

`CollectRemovalCandidates` is the owner-only archiving operation on the meta
socket — the component's only physical-deletion path, ported from old
persona-spirit but moved off the working signal. The meta request carries a
`RemovalCandidateCollection { RecordQuery }` (the candidate selection; the
destination comes from owner config). It runs with NO guardian and NO Nexus
effect: `Engine::collect_removal_candidates` calls
`Store::collect_removal_candidates` directly, which opens the SEPARATE archive
database on demand at the owner-configured `ArchiveDatabaseTarget` (a distinct
`ArchiveDatabase` noun over its own `*.sema` file, resolving `Default` to a
`<live-stem>.archive.sema` sibling), asserts each exact-zero-certainty matching
`Entry` into it, retracts the original from the live log, and returns
`RemovalCandidatesCollection { archived_records, removed_identifiers,
skipped_removal_candidates, database_marker }`. A record that fails to archive
stays in the live log and is reported as a `SkippedRemovalCandidate(ArchiveFailed)`;
one that vanishes mid-collection is `RecordAlreadyRemoved`. The meta op replies
`Output::RemovalCandidatesCollected`.

`Tap`/`Untap` port old persona-spirit's observer (meta-observation) stream as a
request/reply surface. A `Tap(ObserverFilter)` opens at the current operation
revision and emits `CommandEffect(OpenObserverTap(...))`; it replies with an empty
`ObservationTapped(ObserverSubscription)`. Every later admitted operation is
retained as a typed `OperationKind` only while at least one active tap can still
consume it. `Untap(token)` emits `CommandEffect(CloseObserverTap(...))` and
replies `ObservationUntapped(ObserverRetraction)` with that tap's filtered suffix,
then reclaims the prefix no remaining tap can observe. `Watch`/`Unwatch`
reconciliation: old `Watch` (records subscription) is already covered by
`SubscribeIntent`; the un-covered half — token-based cancellation — is what
`Untap` restores. The observer event push-stream
(`OperationReceived`/`EffectEmitted` as live frames) is not wired because the
generated streaming `Frame` carries a single event type (`IntentEvent`); the
active-tap suffix is the load-bearing observer content and is delivered
request/reply.

### Judge admission

Every working-socket write that changes the live intent corpus is gated by the
Spirit judge adapter over the `signal-spirit-judge` typed rkyv socket. The daemon
constructs `JudgeAdmission` and `JudgeReferentRegistration` packets from the
existing Spirit-specific operation nouns, sends them over the judge socket, and
applies only the typed `AdmissionJudged` / `ReferentRegistrationJudged` replies.
The daemon no longer owns prompt prose, model/provider settings, provider JSON,
NOTA verdict parsing, or format-correction mechanics; those belong to
`spirit-judge`, `spirit-judge-config`, and the shared `judge` provider boundary.

The persisted audit and wire names still say `Guardian` where compatibility
requires it: `GuardianOperation`, `GuardianDecision`, `GuardianRejected`, the
Guardian journal filename/family, and the old `SpiritGuardianAgentConfiguration`
startup slot remain narrow compatibility aliases. New source prose and operator
affordances should describe the external component as the judge. The old
configuration slot's socket path is interpreted as the Spirit judge socket; its
provider/model fields are ignored by the daemon because provider selection is an
adapter concern. `GuardianPromptTarget` remains in the meta `Configure` receipt
for wire compatibility, but the daemon only echoes it and does not compile or
apply prompt text.

Judge requests carry an explicit `JudgmentScope`: public candidate entries are
marked `Public`; private entries use `Private(HashesAndRedaction)` so adapter
diagnostics must default to redaction and content hashes. Referent-registration
judgment also uses the private diagnostic posture because referent evidence can
carry private testimony. Any socket error, malformed frame, request rejection,
wrong reply kind, unavailable adapter, or malformed provider response maps
conservatively to the existing harness rejection reasons and fails closed before
any SEMA write. Owner-only `Import` and `CollectRemovalCandidates` remain the
only guardian/judge-bypassing mutation paths.

### SEMA

`Store` is the SEMA writer. SEMA means database work: the SEMA plane writes
durable state to the component database file (records 1007/1008). The store
uses `sema-engine` over a `*.sema` file:

- `Store::open(path)` creates or opens the `.sema` file through
  `sema_engine::Engine`, opts into the versioned commit log with the
  schema-generated `RecordFamily::versioning_policy()`, and registers the
  schema-declared families (`RecordsFamily` over `records`, `ReferentsFamily`
  over `referents`, `MigrationsFamily` over `migrations`) through their
  generated descriptors. The stored shapes (`StoredRecord`, `StoredReferent`,
  the `Migration` marker) are schema nouns in `schema/sema.schema`, not hand
  Rust. Every durable write therefore lands a replayable versioned log entry;
  `Store::checkpoint` folds the log into a content-addressed restore artifact,
  and `Store::import` restores a fresh store from checkpoint + log suffix with
  an identical query surface. The separate archive database and the guardian
  decision journal register family identities but stay UNVERSIONED (no
  policy): both are derived/audit state, not the authoritative intent log.
  **Intent is intent — the guardian is a function that keeps intent clean, not a
  source of it — so `GuardianDecision` is deliberately kept OUT of the versioned
  intent log: its accept/reject verdicts are *about* intent, not intent.** The
  guardian journal is the single sanctioned place a family identity is
  hand-labeled (`SchemaHash::for_label`) instead of schema-derived — a separate
  non-schema audit family legitimately needs a label, and schema-declaring
  `GuardianDecision` would fold the LLM verdict/reasoning types onto the schema
  plane and put non-intent into the intent corpus. Keeping the journal separate
  lets it evolve and be pruned on its own schedule without touching intent
  history.
- `StoreMigration` (feature `production-migration`, binary
  `spirit-migrate-store`) is the schema-version bump path: pre-versioning
  stores (schema versions 7 and 8, unreadable by the current engine's storage
  layout) are read with the renamed `sema-engine-previous` dependency,
  converted through the historical `From`-chain, and written into a fresh
  version-9 store through the logged choke points, closing with the typed
  `Migration` marker — migration as a logged fold, not an unlogged rewrite.
  No pre-version-7 store exists anywhere, so a probed version below 7 is
  rejected as `UnknownSchemaVersion` rather than folded forward.
  The default archive sibling is rebuilt alongside; the v2 guardian journal
  file stays on disk and a fresh v3 journal file starts. Identifier-migration
  data (the hash-to-former-ordinal mapping built while live lookup moves to hash
  identity) is stored as NOTA data, not in a SEMA store: SEMA is for
  schema/engine structure, not for the migration data file itself.
- `production_migration::v13` is a narrower frozen-reader surface for the
  schema-version-13 handoff. `LiveReader` admits exactly the published
  records/referents/migrations catalog; `ArchiveReader` admits exactly the
  records-only archive catalog. Their local archived types reproduce the
  published v13 generated layouts and family hashes, with exact row-byte,
  archived-size/alignment, and enum-discriminant witnesses. Catalog validation
  happens before typed table references are formed, and neither reader exposes
  a source-write method. `FoldSink` is only the typed destination seam: the
  final fold into Ethos-generated current families is incomplete until those
  families exist. Disposable-store tests prove identifier retention, rerun
  stability, wrong-version and corrupt-family refusal, and a partially
  accepting sink while the source inventory, database marker, and catalog stay
  unchanged. The module remains below the `production-migration` feature and is
  absent from daemon, ordinary, owner-meta, and normal dependency surfaces.
- The migration swap is crash-safe with single-rename exposure. The fold
  writes the fresh store beside the live one
  (`<stem>.schema-9-migrating-<pid>.sema`); the swap first hard-links the
  previous store to the backup path (`<stem>.schema-old-backup-<N>.sema`,
  first free `N`), then ONE atomic rename moves the fresh store over the
  live path — the live path is never absent. A crash before the rename
  leaves the previous store live (re-run `spirit-migrate-store`; it sweeps
  every stale `*.schema-9-migrating-*.sema` temporary by glob — not only
  this process's PID — and redoes the fold); a crash after it leaves the
  migrated store live and a re-run reports `Current`. The previous store's
  bytes always survive at the backup path; rollback is stop the daemon and
  copy the newest backup over the live path
  (`cp <stem>.schema-old-backup-<N>.sema <stem>.sema`). The archive sibling
  swaps with the same backup-link plus single-rename pattern.
- `SemaEngine::apply(sema::Sema<sema::WriteInput>) ->
  sema::Sema<sema::WriteOutput>` is the mutation surface. A `Record` becomes
  a keyed assertion: Spirit mints an unused short/base36 string
  `RecordIdentifier`, persists the `Entry`, and advances the durable
  `CommitSequence`. A production migration can instead import a copied
  production identifier unchanged. `ChangeCertainty` and `ChangeRecord` mutate
  the stored `Entry` at that same key and advance the durable sequence once. The
  SEMA `WriteInput` carries no delete variant; keyed retraction comes from
  `Store::retire` and the owner-only meta `CollectRemovalCandidates`, both
  through the low-level `Store::remove` retract, which advances the same durable
  sequence when a record was present.
- `SemaEngine::observe(sema::Sema<sema::ReadInput>) ->
  sema::Sema<sema::ReadOutput>` is the read surface. `Observe(Query)` reads
  keyed records through sema-engine and applies Spirit's schema-specific
  domain/keyword/text/referent/kind/privacy/certainty/importance predicate,
  `PublicTextSearch(SearchText)` performs ranked active-public text lookup and
  returns direct capped results, `Lookup(RecordIdentifier)` uses a key query,
  and `Count(Query)` returns the number of matching records without mutating
  state. `DomainMatch::Any` is the all-record query. The `&self` receiver lets parallel readers share
  the store reference; `tests/runtime_triad.rs` has a scoped-thread witness for
  this shape.
- Entries carry `Domains`, generated `Certainty`, generated `Importance`,
  generated `Privacy`, and `Referents`. Referents are runtime registry data:
  `RegisterReferent` stores a canonical atom plus aliases, writes canonicalize
  aliases before persistence, and queries canonicalize alias filters before
  matching. Privacy is a directional `Magnitude`: `Zero` is open/public, and
  higher magnitudes narrow the intended audience — a nominal level only, not a
  security boundary (see Known limits). Certainty is a directional
  `Magnitude`: `Zero` is the recoverable removal-candidate state. Importance is
  a separate directional `Magnitude` for importance/repetition; observation
  sorts higher importance first. Queries carry `DomainMatch::{Partial,Full}`,
  `KeywordMatch`, `TextMatch`, `ReferentSelection`, an optional `Kind`,
  generated `PrivacySelection`, generated `CertaintySelection`, and generated
  `ImportanceSelection`: `Partial` accepts any requested domain, `Full` requires
  every requested domain, default privacy selection is exact `Zero`, ordinary
  certainty selection is `AtLeastCertainty Minimum`, and ordinary importance
  selection is `Any`. The same
  query noun drives both `Observe` and `Count`, while `Lookup` uses the
  generated `RecordIdentifier` alias.
- sema-engine's transaction model gives crash-consistency: a store reopened
  from the same `.sema` path resumes its committed records AND its commit
  sequence/identifier counters, so the next write after a restart continues
  the sequence rather than restarting at 1.

SEMA replies carry a generated `DatabaseMarker` with `CommitSequence` and
`StateDigest`. `CommitSequence` is the persisted durable write counter.
`StateDigest` is a real content-addressed hash: blake3 over each committed
record's `(identifier, archived bytes)` folded with the commit sequence,
reduced to the schema's `Integer` width — an empty store digests to zero.
Nexus uses that internal marker to close processed mail, but it is database
introspection state, not context for every reply. Ordinary agent-facing Signal
outputs — `Lookup`, `Version`, `Count`, `Observe`, mutation receipts, guardian
rejections, referent registrations, errors, and stream events — omit the marker
tuple. A caller that wants marker state asks for it through the dedicated bare
`Marker` input, which replies `MarkerReported(DatabaseMarker)`.

The database lifecycle is owned by sema-engine. The daemon opens one `Store`
for the process and shares it behind the `Nexus` mutex; `Store` owns the
schema-specific SEMA mapping, while sema-engine owns the database handle,
typed table, durable counters, and commit log.

### Reuse

The schema declares reusable import/export nouns for language planes:
`Import { SourcePath * LocalPath * }` and
`Export { LocalPath * PublicPath * }`.
The paths are single-colon namespaces, mirroring Rust crate/module paths with
`:` instead of `::`, for example `signal:sema:Magnitude`.

The same root shape applies to the Spirit language planes in this pilot:
Signal (`Input`/`Output`), Nexus (`NexusWork`/`NexusAction`), and split SEMA
write/read roots (`WriteInput`/`WriteOutput`, `ReadInput`/`ReadOutput`). Each
plane has imports, roots, and a namespace available to it; the implementation
difference is which actor object owns the method after the generated type
exists.

The current `schema/{signal,nexus,sema}.schema` spelling is the strict brace
key-value syntax with compact enum signatures. The known root positions provide
each plane's input and output enum names, so the root enum bodies are bare
square-bracket values. Namespace declarations are key-value pairs: a brace
value declares a struct map, a square-bracket value declares an enum variant
list, and an atom or parenthesized reference declares an alias. Data-carrying
enum payloads in namespace enum bodies use self-tagged entries such as
`(Record)`, `(RecordAccepted)`, and `(CommandSemaWrite)` when the payload type
has the same name; explicit `(Variant Payload)` remains available when the
names differ. Namespace bindings such as `Record RecordRequest`,
`RecordAccepted RecordIdentifier`, and `CommandSemaWrite [(Record) ...]` define the
payload aliases and feature-specific commands those signatures reference.
Parentheses remain the composite/reference and structural payload shape at
reference positions. That authored syntax decodes to typed
`SchemaSource`, lowers to semantic `Schema`, and emits one generated Rust
module per plane.

The generated Rust exposes plane namespaces over those bootstrap backing names:
`signal::Input`, `nexus::Work`, `sema::WriteInput`, and `sema::ReadInput`
(plus matching output roots). Public execution signatures use the
namespace-local names, for example `sema::Sema<sema::WriteInput>`, so the
envelope carries the plane and payload names stay short.
Those generated roots implement shared `triad-runtime` role traits. The
runner can depend on roles such as `NexusWork`, `NexusAction`,
`SemaWriteInput`, and `SemaReadInput` without reusing Spirit's concrete enum
names as if they were the universal type for every component.

The crate root does not flatten those generated nouns. External code that
needs schema types imports `spirit::schema::signal::Input`,
`spirit::schema::nexus::NexusAction`, or `spirit::schema::sema::WriteInput`.
The crate root exports hand-written runtime composition objects and transport
support only.

When code needs to branch across planes, it matches generated
plane-specific envelopes and actions. There is no generic `schema::Plane`
wrapper in the split runtime; cross-plane movement is explicit through typed
Signal, Nexus, and SEMA envelopes.

## Implementation methods

Schema-generated types are the implementation nouns. Hand-written runtime code
attaches behavior to those nouns or to state-owning runtime objects:

- `Input` is admitted by `SignalAdmission::admit`, producing `SignalAccepted`.
- invalid `Input` is rejected as generated `Output::Rejected(SignalRejection)`
  before mail is sent or SEMA is touched.
- `SignalAccepted` emits `MessageSent` through a hook at the Signal→Nexus
  handoff and asks `SignalAdmission::triage` for generated
  `nexus::Nexus<nexus::Work>`.
- `NexusEngine::execute(&mut Nexus, nexus::Nexus<nexus::Work>)` is the Nexus
  decision boundary. Its mutable borrow enforces one execution at a time on
  the Nexus object.
- `nexus::Nexus<nexus::Work>` becomes generated
  `nexus::Nexus<nexus::Action>`.
- `nexus::Action::CommandSemaWrite` carries the Nexus-plane
  `CommandSemaWrite` feature enum (`Record`, `ChangeCertainty`, `ChangeRecord`);
  `nexus::Action::CommandSemaRead` carries the generated `sema::ReadInput`.
- `sema::Sema<sema::WriteInput>` is applied by `Store` through generated
  `SemaEngine::apply`, writing the durable `.sema` database through
  sema-engine.
- `sema::Sema<sema::ReadInput>` is observed by `Store` through generated
  `SemaEngine::observe`, reading keyed records through sema-engine.
- `sema::Sema<sema::WriteOutput>` or `sema::Sema<sema::ReadOutput>` becomes
  `nexus::Work::SemaWriteCompleted` or `nexus::Work::SemaReadCompleted`, then generated
  `signal::Signal<signal::Output>` carrying a `DatabaseMarker`.
- `MailLedger` holds only sent mail currently in flight, including its
  `OriginRoute`; the matching processed marker removes that entry immediately.

Production-candidate handover is exercised by copying a seeded `.sema`
database file before starting the candidate daemon. The candidate must observe
the copied records, resume the copied ledger for new writes, and leave the
original production-like database unchanged when it is reopened.

- `Input` and `Output` frame themselves at the Signal boundary.

This is the local version of the async mail keeper pattern. `SignalAdmission` is
the Signal admission object, `Nexus` is the data-bearing decision object that
owns the store + ledger, `MailLedger` is the hookable lifecycle sink, and
`Store` is the data-bearing durable SEMA writer/reader. `Engine` composes them.
The generated plane envelopes move between those objects; the code must not
replace that movement with module-level routing helpers or old convenience
wrappers.

Testing trace follows the same ownership rule. `SignalAdmission`, `Nexus`, and
`Store` write typed events into `TraceLog` in `testing-trace` builds. Signal
emits started/stopped plus admitted/triaged/replied, Nexus emits
started/stopped plus entered/decided, and SEMA emits started/stopped plus
write-applied/read-observed. The local trace module implements
`triad_runtime::trace::TraceEventFrame` for `TraceEvent`, renders `TraceEvent`
as generated NOTA in text-client builds, and re-exports the generic runtime
objects.
`triad-runtime` decides whether to record in memory, write a rkyv frame to the
trace socket, or stay disabled when explicitly requested.

When a data shape changes, edit the owning plane schema first, then regenerate
through `build.rs`, then update the methods that act on the regenerated types.
Do not hand-write parallel type mirrors.

## Local stack testing

The flake exposes normal and trace package surfaces:

- `packages.default`, `packages.cli`, and `packages.daemon` are the normal
  runtime pair.
- `packages.trace`, `packages.trace-cli`, and `packages.trace-daemon` are the
  trace-enabled testing pair.
- `checks.test-testing-trace` proves the in-process event sequence.
- `checks.test-testing-trace-process-boundary` proves live CLI/daemon trace
  delivery over Unix sockets.

Process-level tests share `tests/support/process.rs` as their safety boundary.
Every command crossing this boundary starts with an empty environment and
restores only an explicit, reviewed nonsecret variable when a toolchain
executable needs it. The optional Cargo configuration root is accepted only
when Nix owns it below that build's ephemeral `NIX_BUILD_TOP`.
Sockets, binary configuration, databases, archives, and nested build outputs
default to one auto-cleaned temporary directory. Keeping those artifacts
requires a separately constructed manual opt-in with a non-empty reason; no
automated gate uses that path. Managed child handles watch for early exit,
accept readiness only when the filesystem object is an actual Unix socket,
and kill and reap a remaining child before the handle drops.

Candidate-store preparation is also explicit and test-only. The live source
and the archive choice are separate inputs; the archive is either an absolute
path or the exact `absent` choice, never an inferred sibling. Missing manual
configuration is a typed hard error. Preparation refuses symlinks and
non-regular files, copies inputs into a fresh auto-cleaned sandbox, verifies
that candidate and source are distinct filesystem objects, and fingerprints
source bytes and metadata without constructing a Spirit `Store`. Automatic
tests exercise only synthetic disposable source files. A future migrated-copy
acceptance run may reuse these mechanics only with source files that are
already isolated from production; it must remain an explicit manual operation
and must never point a daemon at the source files.

No `last-version` package is exposed yet. That package needs a real previous
release input/tag so upgrade tests compare current code against a previous
artifact rather than an alias of current main.

`scripts/check-local-schema-stack` runs the central local override test for
this pilot. It rebuilds `spirit` with local checkouts of the whole Li
dependency graph that can otherwise appear as Cargo git inputs: `nota`,
`nota-codec`, `nota-derive`, `schema`, `schema`, `schema-rust`,
`sema`, `sema-engine`, `signal-core`, `signal-frame`, `signal-sema`, and
`triad-runtime`. The override set is intentionally complete so a Nix cache miss
does not let Cargo fetch GitHub during the build. This is the intended loop
while improving the NOTA parser, schema lowering, Rust emitter, SEMA engine, or
triad runtime: edit a substrate repo, run the consumer check here, and prove
the generated Rust still compiles and crosses the CLI/daemon rkyv boundary.

`build.rs` delegates the build-time schema pipeline to
`schema_rust::build`. The plan imports the dependency schema exposed by
`signal-spirit` for the ordinary signal/domain contract and the dependency
schema exposed by `meta-signal-spirit` for the owner-only meta policy contract,
then emits the daemon-local modules: `schema/nexus.schema` with `NexusRuntime`,
`schema/sema.schema` with `SemaRuntime`, and the generated daemon runtime. The
shared driver reads each authored daemon-local schema into `SchemaSource`,
round-trips it through `SchemaSourceArtifact` as text and rkyv internal codec
witnesses, lowers from that typed source value, and emits Rust with the opt-in
`nota-text` surface. It compares generated Rust output against
`src/schema/{nexus,sema,daemon}.rs`, or rewrites those files when
`SPIRIT_UPDATE_SCHEMA_ARTIFACTS` is set. Runtime code imports the checked-in
plane modules directly; it does not include generated Rust from `OUT_DIR`.

The same schema-emitted data types can therefore be compiled as binary-only
daemon nouns or as dual NOTA+rkyv CLI nouns without hand-written parallel
mirrors. Cargo feature unification means a single Cargo invocation cannot
prove "CLI has NOTA, daemon lacks NOTA"; Nix builds the daemon and CLI as
separate package derivations and joins their binaries for integration tests.

The schema-rust output paths are already crate-relative
(`src/schema/nexus.rs`, `src/schema/sema.rs`, and `src/schema/daemon.rs`).
`build.rs` uses those paths directly; it does not reinterpret generated paths
relative to `src/`.

Runtime-chain tests assert on schema-emitted objects, not test-local shadow
languages. Pattern A uses Spirit runtime `MailLedgerEvent` plus generated
`NexusWork`, `NexusAction`, `SemaWriteInput`, `SemaWriteOutput`,
`SemaReadInput`, and `SemaReadOutput` as witnesses. The SEMA engine tests call generated
`SemaEngine::apply(sema::Sema<sema::WriteInput>) ->
sema::Sema<sema::WriteOutput>` and
`SemaEngine::observe(sema::Sema<sema::ReadInput>) ->
sema::Sema<sema::ReadOutput>` against a real `.sema` file, so each runtime
plane remains typed as its schema at both ends of the operation. The
process-boundary tests parse the CLI's stdout back through schema-emitted
`Output::FromStr` (no raw-string digest assertions, since the digest is now a
real content hash), and one of them proves durability at the real process
boundary: a daemon writes the `.sema` file, the process is killed, a fresh
daemon opens the same `.sema` file, and the recorded entry is still observable
with the commit sequence resumed. A dedicated store-reopen test in
`runtime_triad.rs` proves the same durability at the library level.

The optional `testing-trace` feature adds an in-process structured trace sink
for instrumentation tests. It is compiled out of default and `nota-text`
production builds. When enabled, `TraceEvent` values are emitted from the live
Signal admission/reply, Nexus execution/decision, and SEMA apply/observe call
sites. `tests/instrumentation_logging.rs` installs a `TraceLog`, drives the
normal runtime through `Engine::handle`, and asserts the schema-generated typed
event sequence instead of grepping for trait names.

The next larger migration candidate is the workspace split proven in the
designer worktree: separate working-signal, meta-signal, engine, daemon, and
CLI crates, with meta-signal carrying policy/configuration operations and a
runtime-level numerator enum over accepted signal interfaces. Main currently
keeps the single crate so the Nix proof harness remains intact; the integrated
pieces from that prototype are the zero-NOTA dependency guard and raw-NOTA
socket rejection tests.

## Design direction

These are accepted directions not yet built into the current crate; they shape
where the component is headed.

- **Two stacks coexist during cutover.** `persona-spirit` is the deployed
  production daemon, this schema-derived stack the pilot. The production
  component carries a clear, prominent in-code production marker so agents patch
  the right repository while both coexist, and remaining side-by-side legacy
  daemon slots stay fixed to run. The naming direction is to shed unnecessary
  persona ancestry: Spirit stands as the component name, `signal-spirit` as its
  ordinary signal layer, and owner terminology moves toward core terminology for
  the privileged surface.
- **Versioned sockets and signal-version handover.** Ordinary and owner sockets
  stay separate, with permissions enforced by filesystem socket access under
  trusted local development. Parallel daemon versions need versioned sockets: a
  multi-version dispatch protocol routes the CLI to the next-version daemon,
  which coordinates back with main, and a missing next-version returns a logged
  error. Upgrade orders that trigger the smart handover arrive through the
  component's owner socket, and `signal-version-handover` is the single discovery
  mechanism for next-version daemons (the old `PeerCheck` is retired). The
  longer-horizon schema-migration shape is in-process versioned reads: every
  record carries a schema-version tag, the daemon links every prior version's
  types and dispatches read-side on the tag, migrating older records on read for
  zero downtime per bump, with a per-type migration trait that knows whether to
  consult the next-version daemon over the version-handover protocol.
- **A closed data-lifecycle ladder.** The lifecycle operations form a closed
  named set worth building as one concept — nominate, tombstone, archive,
  collect, compact, purge — rather than ad-hoc operations. It lands on this
  schema-derived stack, not the hand-written production daemon, because it is a
  large feature.
- **Federation of key-gated stores.** Private component state is organized as
  multiple key-gated stores on the GoPass model: each store is encrypted to a set
  of recipient keys, access is key possession, and the per-store key is both the
  access boundary and the crypto-shred erasure unit (accountable content erasure
  via crypto-shredding with a signed erasure receipt). The exploration is to
  rethink Spirit as a federation of key-scoped stores that multiple instances
  share as ciphertext, with queries scoped to the stores whose keys the instance
  holds. This is the secure-private answer to the nominal-privacy limit below.
- **Per-psyche intake.** The intent recorder uses voice recognition as the cutoff
  between recorded-and-processed and ignored audio, defaulting to ignore audio
  not recognized as the psyche. When the active microphone passes to another
  speaker, the recorder recognizes the voice change and routes that speech to
  that speaker's own dynamically-spawned instance — captured intent belongs to
  each person as a distinct psyche with their own intent layer.

## Known limits

- Privacy is NOMINAL / name-only, not a security boundary. The `Privacy`
  `Magnitude` records a level — `Zero` is public, any nonzero is private —
  inside ONE database with no real security fail-safe: no encryption, no
  storage segregation, no enforced access gate. Treat all Spirit data as
  potentially exposed. Genuinely sensitive content goes to actually-private
  storage (private repositories), not Spirit; the privacy level only narrows
  the intended audience, it does not protect it. The feature may later be
  implemented as secure-private; until then a nonzero rung is a label, not a
  seal.
- The mail ledger is in-memory current state, not durable observability history:
  in-flight sent mail resets on daemon restart and terminal mail is reclaimed.
  Only the SEMA records and commit ledger are durable.
- Schema diff/upgrade is absent (the generated `UpgradeFrom`/`AcceptPrevious`
  traits exist but nothing implements them yet).
- The repo-triad split (`spirit`, `signal-spirit`, `meta-signal-spirit`) is now
  present for both external contracts: the daemon imports the ordinary
  signal/domain contract and `SpiritDaemonConfiguration` from `signal-spirit`
  and imports owner-only meta-policy wire nouns from `meta-signal-spirit`.
- `MessageSent` and `MessageProcessed` are Spirit runtime mail-ledger types,
  not ordinary signal contract nouns. The `Nexus` decision object and SEMA
  `Store` are hand-written runtime behaviour over the schema-emitted nouns;
  they are not boundary types and stay hand-written.
- `Store` still lives inside the `Nexus` mutex rather than a kameo
  single-writer actor. The database boundary is now sema-engine; the remaining
  question is runner/actor ownership, not raw storage access.
- The next slice should make the mail support schema-authored, move the durable
  marker toward a shared `schema-core` type, and start schema diff/upgrade.
