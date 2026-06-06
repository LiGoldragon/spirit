# ARCHITECTURE — spirit

## Purpose

`spirit` is the running proof that schema can create an interface used by
a real CLI and daemon pair.

It is also the current copyable exemplar for the schema-derived triad engine
stack. The repo is intentionally one daemon crate, but it is not an all-in-one
schema shape: the runtime planes are split into `schema/signal.schema`,
`schema/nexus.schema`, and `schema/sema.schema`, generated through the shared
driver into `src/schema/{signal,nexus,sema}.rs`, and consumed through
`triad-runtime` plus `sema-engine`. Future component daemon repos should copy
this plane/runtime shape while placing their external ordinary/meta signal
contracts in separate contract repos where rebuild and policy boundaries
require it.

## Layers

```text
schema/{signal,nexus,sema}.schema
  -> build.rs
  -> schema_rust_next::build::GenerationPlan with three ModuleEmission targets
  -> schema_rust_next::build::GenerationDriver
  -> schema-next::SchemaSource typed source objects inside the shared driver
  -> rkyv-serializable schema-in-Rust values checked by the shared driver
  -> schema-rust-next::RustEmitter with opt-in NOTA surface inside the driver
  -> checked-in generated modules at src/schema/{signal,nexus,sema}.rs
  -> engine composer + nexus mail keeper + sema-engine backed store + transport
```

Each generated module has one binary floor and one optional text surface.
`rkyv::Archive` / `Serialize` / `Deserialize` are always emitted because every
component speaks binary frames. `nota_next::NotaDecode` / `NotaEncode`, root
`FromStr`, root `Display`, and `to_nota` helpers are emitted behind the
`nota-text` feature. That lets the CLI crate target parse and print NOTA while
the daemon target compiles without the NOTA decoder linked into its runtime
surface. `tests/dependency_surface.rs` is the executable guard: the normal
dependency tree with `--no-default-features` must contain no `nota-next`, while
the `nota-text` tree must contain it.

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
maps. The Signal namespace contains pairs such as `Topic String`,
`RecordSet (Vec Entry)`, and
`Entry { Topics * Kind * Description * Magnitude * Privacy * }`; it does not
contain declarations that repeat their own name inside the value.
Inside a struct map, `Topics *` derives the `topics` field from the existing
`Topics` type, and explicit bindings such as `kind (Optional Kind)` stay only
where the field name differs from the referenced type. Bare reference
declarations (`Topic String`, `RecordSet (Vec Entry)`, `Record Entry`) become
exported aliases in the typed schema value and generated Rust, so enum variants
carry direct payloads instead of wrapper structs. Explicit brace-body singleton
declarations are the newtype form.

Enum bodies keep vector homogeneity by listing exported object names. Namespace
bindings such as `Record Entry`, `RecordAccepted SemaReceipt`, and
`SignalArrived Input` define the payload shape for data-carrying objects; names
without payload bindings are unit variants. The vector does not contain pseudo
key-value pairs or parenthesized root signatures.

The three runtime centers are concrete objects: `SignalActor` (admission),
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
`Daemon` then constructs `SpiritDaemonRuntime` around the component `Engine`
and gives it to `triad_runtime::MultiListenerDaemon` as one or two tagged
`ListenerSocket`s (the working socket always, the owner-only meta socket when
`meta_socket_path` is set). The shared runtime prepares each Unix socket, binds
the listeners, starts the runtime, serves accepted streams round-robin, logs
request-level errors without stopping the listeners, and stops the runtime when
the accept loop exits. Spirit's runtime object owns only
engine construction and the one-stream generated signal-frame bridge.

This is the live startup-runner slice: startup and listener behavior belongs to
shared runtime nouns, while domain decisions belong to generated engine trait
implementations.

## Borrowed prototype lessons

- From `design-nota-from-schema`: make the recursion floor explicit and keep
  generated source as source, not hidden macro behavior.
- From operator `schema-rust-next`: schema emits Rust code first; Rust macros
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

The CLI parse path is generated-first. The only compatibility shim currently
accepted at this edge is deployed production `State` shorthand:
`(State ([text]))`. `SpiritInputSource` recognizes that exact structural shape
with `nota-next` blocks, creates generated `Input::State(Statement)`, and then
the normal binary signal path continues. The generated canonical form remains
`(State [text])`; the daemon never receives either text spelling.

The daemon:

1. starts from `DaemonCommand`, which accepts exactly one argument: a path to a
   binary rkyv `Configuration` object;
2. constructs `SpiritDaemonRuntime`, opening the configured `.sema` database
   path through `Store`;
3. hands that runtime and the configured socket paths (working, plus the
   owner-only meta socket when `meta_socket_path` is set) to
   `triad_runtime::MultiListenerDaemon`;
4. starts the generated engine lifecycle through `MultiListenerRuntime::start`;
5. reads a length-prefixed binary frame from each accepted stream;
6. asks generated `Input` to triage by short header and decode itself (the
   working socket decodes signal `Input`; the meta socket decodes meta-signal
   `Input`);
7. dispatches through `Engine`;
8. asks generated `Output` to frame itself as binary rkyv;
9. writes it back.

The daemon does not parse NOTA at startup and does not need `nota-next` for
its binary-only build. A text launcher or test can write the binary
configuration file; production configuration should later become another
typed binary signal surface differentiated by the root message enumerator,
not a NOTA side channel.

The binary configuration carries the daemon's meta slot as an optional
`meta_socket_path`. When that path is set, the daemon binds a SECOND listener
on it — the owner-only meta-signal surface — distinct from the ordinary working
socket, so policy and configuration authority have a component-owned home apart
from the peer-callable working signal. The meta contract is the crate-local
`schema/meta-signal.schema` wire-only module (a fourth schema module emitted via
`RustEmissionTarget::WireContract` into `src/schema/meta_signal.rs`): it carries
only the `Configure` `Input` root, the `Configured`/`Rejected` `Output` roots,
their records, and the rkyv derives — no Nexus/SEMA planes and no engine traits.
The single owner-only operation is
`Configure(ConfigureRequest { ArchiveDatabaseTarget })`, where
`ArchiveDatabaseTarget` is the ported `[Default | Path(ArchivePath)]` enum. It
sets WHERE the SEPARATE archive database lives — the destination the
peer-callable `CollectRemovalCandidates` working operation archives into. The
archive database is a distinct `*.sema` file from the live intent log;
`Configure` does NOT open, move, or touch the live database. The daemon applies
it through `Engine::configure` (an owner-config effect under the same
single-flight Nexus mutex the working path uses, NOT a SEMA log write), which
stores the target on the SEMA `Store` (`Store::set_archive_target`, a field +
accessor) and replies with the now-active target plus the live database marker.
The authority split is load-bearing: the OWNER configures WHERE archives go
(meta `Configure`); a PEER does the archiving (working
`CollectRemovalCandidates`). The earlier `set_archive_target` re-opened the LIVE
`SemaDatabase` at the new path — that was the bug; the live log is now never
disturbed by a reconfigure.

The daemon binds both sockets through `triad_runtime::MultiListenerDaemon`,
tagging each with a `SpiritListener` discriminant (`Working` / `Meta`) so
`MultiListenerRuntime::handle_stream` decodes the correct wire contract for the
arriving socket. Owner-only authority rests on the meta socket file mode
(`0o600`, applied via `SocketMode`); `triad-runtime` has no peer-credential
check, so the filesystem mode IS the owner gate. When `meta_socket_path` is
`None` the daemon binds only the working socket, preserving the single-socket
lifecycle unchanged. The meta wire codec (an 8-byte short-header frame
byte-identical to the signal plane's) lives on the generated `Input`/`Output`
nouns in `src/meta_transport.rs`, because the `WireContract` emission target in
the current `schema-rust-next` pin emits the per-root `short_header` constants
but not the `encode_signal_frame`/`decode_signal_frame` frame codec.

`Configuration` may also carry a trace socket path. That field is still binary
rkyv configuration; it does not add a NOTA startup path to the daemon. Only a
daemon compiled with `testing-trace` uses it. The daemon writes trace frames as
binary objects, and only the trace-enabled CLI decodes them into text.

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

The daemon handles a subscribe request by writing the ordinary
`SubscriptionStarted` reply, registering the cloned server-side socket writer
under the Nexus-minted `SubscriptionToken`, and returning to the accept loop.
This keeps the single listener non-blocking for other requests while the
retained writer remains available for pushed events. `SubscriptionHub` owns the
live `SubscriptionRegistry`, stream-event publisher, and retained writers;
daemon code only registers subscriptions and asks the hub to publish typed
events.

### Nexus

`SignalActor::admit` is the Signal validation/admission point. It mints the
origin route, issues the generated `MessageSent` event, validates generated
`Input`, and produces `SignalAccepted`. `SignalAccepted::process_with` is the
flat composition point: it emits the sent event, asks `SignalEngine::triage` for
a generated `nexus::Nexus<nexus::Work>`, runs `NexusEngine::execute`, emits
`MessageProcessed<Output>`, and asks `SignalEngine::reply` for the generated
Signal output.

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
communication boundary.

Nexus write commands are feature-specific. `CommandSemaWrite` is not a raw
pass-through alias for the whole SEMA write enum; it is a Nexus-plane enum with
visible `Record`, `Remove`, and `ChangeCertainty` objects. The generated runner
still treats it as the fixed `SemaWrite` outcome, but the component schema
keeps each internal write feature readable in `schema/nexus.schema`.

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

`ChangeCertainty` is the first production conditional-write parity slice.
Signal admits the generated `CertaintyChange` payload. Nexus emits the
schema-declared `CommandSemaWrite(ChangeCertainty(...))`. SEMA looks up the
identified record, mutates only the stored entry's `Magnitude` through the
`Certainty` alias, writes it back through `Engine::mutate_identified`, and
returns `CertaintyChangeReceipt` with the same `RecordIdentifier` and a new
database marker.

`CollectRemovalCandidates` is the peer-callable archiving operation ported from
old persona-spirit. Signal admits a `RemovalCandidateCollection { RecordQuery }`
(the peer supplies only the candidate selection; the destination comes from
owner config). Nexus emits the schema-declared
`CommandEffect(CollectRemovalCandidates(...))`; the effect calls
`Store::collect_removal_candidates`, which opens the SEPARATE archive database on
demand at the owner-configured `ArchiveDatabaseTarget` (a distinct `ArchiveDatabase`
noun over its own `*.sema` file, resolving `Default` to a `<live-stem>.archive.sema`
sibling), asserts each matching `Entry` into it, retracts the original from the
live log, and returns `RemovalCandidatesCollection { archived_records,
removed_identifiers, skipped_removal_candidates, database_marker }`. A record that
fails to archive stays in the live log and is reported as a
`SkippedRemovalCandidate(ArchiveFailed)`; one that vanishes mid-collection is
`RecordAlreadyRemoved`. Nexus replies `Output::RemovalCandidatesCollected`.

`Tap`/`Untap` port old persona-spirit's observer (meta-observation) stream as a
request/reply surface. Every admitted working operation is recorded in the
`ObserverTapTable` operation log as a typed `OperationKind`. `Tap(ObserverFilter)`
emits `CommandEffect(OpenObserverTap(...))`, mints an observer token, and replies
`ObservationTapped(ObserverSubscription)` carrying the operations observed so far
filtered by the `[All | OperationsOnly | EffectsOnly]` filter. `Untap(token)`
emits `CommandEffect(CloseObserverTap(...))` and replies
`ObservationUntapped(ObserverRetraction)` with the tap's final filtered
observations, retiring the subscription. `Watch`/`Unwatch` reconciliation:
old `Watch` (records subscription) is already covered by `SubscribeIntent`; the
un-covered half — token-based cancellation — is what `Untap` restores. The
observer event push-stream (`OperationReceived`/`EffectEmitted` as live frames)
is not wired because the generated streaming `Frame` carries a single event type
(`IntentEvent`); the operation history is the load-bearing observer content and
is delivered request/reply.

### SEMA

`Store` is the SEMA writer. SEMA means database work: the SEMA plane writes
durable state to the component database file (records 1007/1008). The store
uses `sema-engine` over a `*.sema` file:

- `Store::open(path)` creates or opens the `.sema` file through
  `sema_engine::Engine` and registers the identified `records` family.
- `SemaEngine::apply(sema::Sema<sema::WriteInput>) ->
  sema::Sema<sema::WriteOutput>` is the mutation surface. A `Record` becomes
  `Engine::assert_identified`, so sema-engine allocates the numeric
  `RecordIdentifier`, persists the `Entry`, and advances the durable
  `CommitSequence`. A `ChangeCertainty` becomes
  `Engine::mutate_identified`, preserving the numeric identifier while
  replacing the stored `Entry` value and advancing the durable sequence once.
  A `Remove` becomes `Engine::retract_identified`, deleting the identified
  record and advancing the same durable sequence when a record was present.
- `SemaEngine::observe(sema::Sema<sema::ReadInput>) ->
  sema::Sema<sema::ReadOutput>` is the read surface. `Observe(Query)` reads
  identified records through sema-engine and applies Spirit's schema-specific
  topic/kind/privacy predicate, `Lookup(RecordIdentifier)` uses
  `IdentifiedQueryPlan::identifier`, and `Count(Query)` returns the number of
  matching records without mutating state. The `&self` receiver lets parallel
  readers share the store reference; `tests/runtime_triad.rs` has a
  scoped-thread witness for this shape.
- Entries carry `Topics`, a generated vector alias, plus generated
  `Privacy`. Privacy is a directional `Magnitude`: `Zero` is open/public, and
  higher magnitudes narrow the intended audience. Queries carry
  `TopicMatch::{Partial,Full}`, an optional `Kind`, and generated
  `PrivacySelection`: `Partial` accepts any requested topic, `Full` requires
  every requested topic, `None` in the kind position searches by topic and
  privacy, and default privacy selection is exact `Zero`. The same query noun
  drives both `Observe` and `Count`, while `Lookup` uses the generated
  `RecordIdentifier` alias.
- sema-engine's transaction model gives crash-consistency: a store reopened
  from the same `.sema` path resumes its committed records AND its commit
  sequence/identifier counters, so the next write after a restart continues
  the sequence rather than restarting at 1.

SEMA replies carry a generated `DatabaseMarker` with `CommitSequence` and
`StateDigest`. `CommitSequence` is the persisted durable write counter.
`StateDigest` is a real content-addressed hash: blake3 over each committed
record's `(identifier, archived bytes)` folded with the commit sequence,
reduced to the schema's `Integer` width — an empty store digests to zero.
Signal outputs include the state marker that Nexus uses to close processed
mail.

The database lifecycle is owned by sema-engine. The daemon opens one `Store`
for the process and shares it behind the `Nexus` mutex; `Store` owns the
schema-specific SEMA mapping, while sema-engine owns the database handle,
identified table, durable counters, and commit log.

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
key-value syntax. The known root positions provide each plane's input and
output enum names, so the root enum bodies are bare square-bracket values.
Namespace declarations are key-value pairs: a brace value declares a struct
map, a square-bracket value declares an enum variant list, and an atom or
parenthesized reference declares an alias. Data-carrying enum payloads are
declared explicitly in namespace enum bodies with parenthesized entries such
as `(Record Record)`, `(RecordAccepted RecordAccepted)`, and
`(CommandSemaWrite CommandSemaWrite)`. Namespace bindings such as
`Record Entry`, `RecordAccepted SemaReceipt`, and
`CommandSemaWrite [(Record Record) ...]` define the payload aliases and
feature-specific commands those signatures reference. Parentheses remain the
composite/reference and structural payload shape at reference positions. That
authored syntax decodes to typed
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

- `Input` is admitted by `SignalActor::admit`, producing `SignalAccepted`.
- invalid `Input` is rejected as generated `Output::Rejected(SignalRejection)`
  before mail is sent or SEMA is touched.
- `SignalAccepted` emits `MessageSent` through a hook at the Signal→Nexus
  handoff and asks `SignalEngine::triage` for generated
  `nexus::Nexus<nexus::Work>`.
- `NexusEngine::execute(&mut Nexus, nexus::Nexus<nexus::Work>)` is the Nexus
  decision boundary. Its mutable borrow enforces one execution at a time on
  the Nexus object.
- `nexus::Nexus<nexus::Work>` becomes generated
  `nexus::Nexus<nexus::Action>`.
- `nexus::Action::CommandSemaWrite` carries the Nexus-plane
  `CommandSemaWrite` feature enum (`Record`, `Remove`, `ChangeCertainty`);
  `nexus::Action::CommandSemaRead` carries the generated `sema::ReadInput`.
- `sema::Sema<sema::WriteInput>` is applied by `Store` through generated
  `SemaEngine::apply`, writing the durable `.sema` database through
  sema-engine.
- `sema::Sema<sema::ReadInput>` is observed by `Store` through generated
  `SemaEngine::observe`, reading identified records through sema-engine.
- `sema::Sema<sema::WriteOutput>` or `sema::Sema<sema::ReadOutput>` becomes
  `nexus::Work::SemaWriteCompleted` or `nexus::Work::SemaReadCompleted`, then generated
  `signal::Signal<signal::Output>` carrying a `DatabaseMarker`.
- `MailLedgerEvent` stores sent and processed mail markers, including their
  `OriginRoute`, in the ledger Nexus owns.

Production-candidate handover is exercised by copying a seeded `.sema`
database file before starting the candidate daemon. The candidate must observe
the copied records, resume the copied ledger for new writes, and leave the
original production-like database unchanged when it is reopened.

- `Input` and `Output` frame themselves at the Signal boundary.

This is the local version of the async mail keeper pattern. `SignalActor` is
the Signal admission object, `Nexus` is the data-bearing decision object that
owns the store + ledger, `MailLedger` is the hookable lifecycle sink, and
`Store` is the data-bearing durable SEMA writer/reader. `Engine` composes them.
The generated plane envelopes move between those objects; the code must not
replace that movement with module-level routing helpers or old convenience
wrappers.

Testing trace follows the same ownership rule. The generated `SignalEngine`,
`NexusEngine`, and `SemaEngine` traits own default no-op trace hooks and
default wrapper methods around their inner behavior methods. `SignalActor`,
`Nexus`, and `Store` override those hooks in `testing-trace` builds and write
events into `TraceLog`. Signal emits started/stopped plus
admitted/rejected/triaged/replied, Nexus emits started/stopped plus
entered/decided, and SEMA emits started/stopped plus
write-applied/read-observed. The local
trace module only implements `triad_runtime::trace::TraceEventFrame` for the
generated `TraceEvent`, renders `TraceEvent` as generated NOTA in text-client
builds, and re-exports the generic runtime objects.
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

No `last-version` package is exposed yet. That package needs a real previous
release input/tag so upgrade tests compare current code against a previous
artifact rather than an alias of current main.

`scripts/check-local-schema-stack` runs the central local override test for
this pilot. It rebuilds `spirit` with local checkouts of the whole Li
dependency graph that can otherwise appear as Cargo git inputs: `nota-next`,
`nota-codec`, `nota-derive`, `schema`, `schema-next`, `schema-rust-next`,
`sema`, `sema-engine`, `signal-core`, `signal-frame`, `signal-sema`, and
`triad-runtime`. The override set is intentionally complete so a Nix cache miss
does not let Cargo fetch GitHub during the build. This is the intended loop
while improving the NOTA parser, schema lowering, Rust emitter, SEMA engine, or
triad runtime: edit a substrate repo, run the consumer check here, and prove
the generated Rust still compiles and crosses the CLI/daemon rkyv boundary.

`build.rs` delegates the build-time schema pipeline to
`schema_rust_next::build`. The plan emits three modules:
`schema/signal.schema` with `SignalRuntime`, `schema/nexus.schema` with
`NexusRuntime`, and `schema/sema.schema` with `SemaRuntime`. The shared driver
reads each authored schema into `SchemaSource`, round-trips it through
`SchemaSourceArtifact` as text and rkyv internal codec witnesses, lowers from
that typed source value, and emits Rust with the opt-in `nota-text` surface. It
compares generated Rust output against `src/schema/{signal,nexus,sema}.rs`, or
rewrites those files when `SPIRIT_UPDATE_SCHEMA_ARTIFACTS` is set. Runtime code
imports the checked-in plane modules directly; it does not include generated
Rust from `OUT_DIR`.

The same schema-emitted data types can therefore be compiled as binary-only
daemon nouns or as dual NOTA+rkyv CLI nouns without hand-written parallel
mirrors. Cargo feature unification means a single Cargo invocation cannot
prove "CLI has NOTA, daemon lacks NOTA"; Nix builds the daemon and CLI as
separate package derivations and joins their binaries for integration tests.

The schema-rust output paths are already crate-relative
(`src/schema/signal.rs`, `src/schema/nexus.rs`, and `src/schema/sema.rs`).
`build.rs` uses those paths directly; it does not reinterpret generated paths
relative to `src/`.

Runtime-chain tests assert on schema-emitted objects, not test-local shadow
languages. Pattern A uses generated `MailLedgerEvent`, `NexusWork`,
`NexusAction`, `SemaWriteInput`, `SemaWriteOutput`, `SemaReadInput`, and
`SemaReadOutput` as witnesses. The SEMA engine tests call generated
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

## Known limits

- The mail ledger is still in-memory (it is observability, not durable state):
  the `MailLedgerEvent` history resets on daemon restart. Only the SEMA records
  and commit ledger are durable.
- Schema diff/upgrade is absent (the generated `UpgradeFrom`/`AcceptPrevious`
  traits exist but nothing implements them yet).
- The repo-triad split (`spirit`, `signal-spirit`, `meta-signal-spirit`) is
  not represented in this pilot repo.
- `MessageSent` and `MessageProcessed` are generated by the Rust emitter's
  support surface rather than authored in a shared core schema. The `Nexus`
  decision object and SEMA `Store` are hand-written runtime behaviour over the
  schema-emitted nouns; they are not boundary types and stay hand-written.
- `Store` still lives inside the `Nexus` mutex rather than a kameo
  single-writer actor. The database boundary is now sema-engine; the remaining
  question is runner/actor ownership, not raw storage access.
- The next slice should make the mail support schema-authored, move the durable
  marker toward a shared `schema-core` type, and start schema diff/upgrade.
