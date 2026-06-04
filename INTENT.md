# INTENT — spirit

`spirit` exists to prove a running Spirit-like component can be built
from schema-derived interfaces.

It is intentionally separate from production `spirit`/`persona-spirit` so
operators can iterate without disturbing the deployed intent substrate.

Load-bearing constraints:

- CLI input and output are NOTA when the `nota-text` feature is enabled.
- Component/process communication is always binary rkyv.
- Generated schema datatypes always carry rkyv support. NOTA encode/decode is
  an opt-in text-client surface, not a daemon requirement. The daemon build is
  binary-only and must not depend on `nota-next`; the CLI build opts into
  `nota-text` because it is the text-facing adapter.
- The binary-only dependency boundary is an executable contract, not just a
  source comment. Tests run `cargo tree --edges normal --no-default-features`
  and assert `nota-next` is absent, while the `nota-text` surface must contain
  `nota-next`.
- Rust data types are generated from the crate-local `schema/lib.schema`
  entrypoint and materialized as checked-in source under `src/schema/`.
- The macro-free assembled form is materialized as checked-in
  `schema/lib.asschema` text. Build code compares it against the fresh lowering
  of `schema/lib.schema`, then emits Rust through the shared
  `schema_rust_next::build` driver.
- Authored schema source is also a typed artifact before assembly.
  The shared generation driver reads `schema/lib.schema` into `SchemaSource`,
  round-trips canonical source text through `SchemaSourceArtifact`, lowers the
  recovered typed source into `Asschema`, and compares the generated
  `.asschema` and Rust artifacts with the checked-in files. The source
  language therefore has an in/out codec on the Spirit stack instead of being a
  one-way parser.
- `schema/lib.schema` preserves NOTA brace semantics: braces are key-value
  maps, not collections of one-object declarations. A namespace entry is a
  pair such as `Topic String`,
  `Entry { Topics * Kind * Description * Magnitude * Privacy * }`, or
  `Kind [...]`.
  Struct fields are also key-value pairs; `Topics *` means the key names the
  field and `*` reuses the same type, while `kind (Optional Kind)` binds a
  field to a different composite reference. Enum bodies are square-bracket
  lists of exported object names. Namespace bindings such as `Record Entry`,
  `RecordAccepted SemaReceipt`, and `SignalArrived Input` define the payload
  shape for data-carrying objects; names without payload bindings are unit
  variants.
  Bare reference declarations such as `Topic String`,
  `Topics (Vec Topic)`, and `Record Entry` lower to exported aliases and
  direct enum payloads, not wrapper structs. Explicit brace-body singleton
  declarations remain the source form for real tuple newtypes. The generated
  `Asschema` and emitted Rust consume only the strict authored surface.
- The generated file path from schema-rust is crate-relative
  `src/schema/lib.rs`; build code uses that path directly instead of treating
  `schema/lib.rs` as relative to `src/`.
- Schema lowering goes through `schema-next` before Rust emission; build and
  runtime tests prove the generated `Asschema` data and emitted Rust, not a
  macro trace side channel. The build freshness path materializes
  `AsschemaArtifact` as legal `.asschema` NOTA, compares it with checked-in
  `schema/lib.asschema`, and compares emitted Rust with `src/schema/lib.rs`.
  The checked-in generated Rust is produced from typed assembled-schema data
  rather than a private parser side channel.
- `build.rs` is a freshness witness for the generated source. It regenerates
  in memory and fails if `src/schema/lib.rs` is missing or stale.
- Old design-convenience APIs do not remain beside the working interface
  (Spirit record 1339). Once the schema-derived trait path exists, parallel
  bypass/convenience surfaces are removed rather than carried for comfort.
- Daemon startup should move toward a generated/programmatic triad runner.
  The binary `main` is only an entrypoint; argument handling, binary
  configuration loading, daemon construction, and the eventual component
  runner surface belong on data-bearing library nouns. Domain behavior stays
  in non-default implementations of the generated Signal, Nexus, and SEMA
  engine traits, not in daemon main or local orchestration conveniences.
- The schema declares the runtime triad surfaces:
  `Input`/`Output` for Signal, `NexusWork`/`NexusAction` for Nexus decision flow,
  `SemaWriteInput`/`SemaWriteOutput` for database mutations, and
  `SemaReadInput`/`SemaReadOutput` for database reads.
- Generated plane namespaces expose the same shapes as `signal::Input` /
  `signal::Output`, `nexus::Work` / `nexus::Action`,
  `sema::WriteInput`, `sema::WriteOutput`, `sema::ReadInput`, and
  `sema::ReadOutput`. The flat names are bootstrap backing names; runtime
  trait signatures and tests use the plane namespaces so the plane carries
  the ancestry instead of every payload name.
- Cross-plane branching uses generated `schema::Plane::{Signal,Nexus,Sema}`.
  Each variant carries the actual plane envelope; there is no parallel
  `Kind` tag that must be paired with a separate message body.
- Each language plane has input/output and reusable import/export vocabulary.
  Import/export paths mirror Rust module namespaces with a single colon rather
  than double colon, for example `signal:sema:Magnitude`.
- Signal, Nexus, and SEMA use the same authored schema shape: imports/exports,
  roots, and namespace. Their generated Rust differs by trait support and
  runtime ownership, not by a separate notation.
- Nexus is the execution plane between Signal and SEMA. Signal triage produces
  a generated `nexus::Nexus<nexus::Work>` envelope directly; that envelope is
  the only Signal-to-Nexus runtime handoff.
- Heavier topic-discovery and ranking algorithms belong to Nexus as
  non-default decision implementations over generated root messages. The
  durable tables and indexes those algorithms need belong to SEMA; Signal only
  accepts, validates, correlates, and replies.
- Nexus is a real runtime object (`Nexus`), not a set of methods on the
  orchestrator. It owns the durable SEMA store handle and the mail ledger. The
  mutable `NexusEngine::execute(&mut self, nexus::Nexus<nexus::Work>)`
  borrow is the single-flight guard. `Engine` is a thin composer of the three
  centers and never calls the store directly.
- Signal admission is explicit in the pilot. `SignalActor::admit` mints the
  origin route, validates generated `Input`, and creates `SignalAccepted`;
  `SignalAccepted::process_with` fires the generated `MessageSent` hook before
  triaging into Nexus.
- Signal rejection is also schema-emitted. Invalid Signal input returns
  `Output::Rejected(SignalRejection { validation_error, database_marker })`
  where `ValidationError` is generated from `schema/lib.schema`; the runtime
  does not use a hand-written rejection enum at the wire boundary.
- Nexus decides from generated `NexusWork` facts and emits generated
  `NexusAction` commands. Signal arrivals become command actions such as
  `CommandSemaWrite` or `CommandSemaRead`; SEMA completions re-enter as
  `SemaWriteCompleted` or `SemaReadCompleted`; Nexus-local effects re-enter
  as `EffectCompleted`; only `ReplyToSignal` exits back to Signal.
- The recursive runner loop is generated/shared runtime behavior, not local
  Nexus boilerplate. `schema-rust-next` emits the `NexusAction` to
  `triad_runtime::NextStep` projection plus the data-bearing runner adapter;
  `triad-runtime::Runner` owns the continuation budget and repeated dispatch.
  Hand-written `Nexus` implements one decision step, SEMA write/read hooks,
  the effect hook, and the typed budget-exhausted reply.
- `schema-rust-next` emits `SignalEngine`, `NexusEngine`, and `SemaEngine`
  when the schema declares the corresponding input/output pairs. `SignalActor`
  implements `SignalEngine` as the lightweight triage/reply boundary; `Nexus`
  implements mutable `NexusEngine` as the computation and decision plane;
  `Store` implements `SemaEngine` as the durable state plane with split
  `apply(&mut self, WriteInput)` and `observe(&self, ReadInput)` surfaces;
  tests call those trait surfaces with generated schema root objects.
- The generated engine traits also provide the minimal lifecycle address.
  `Engine::start` runs the generated hooks from inner durable state outward
  (SEMA, then Nexus, then Signal), and `Engine::stop` runs them from the
  communication boundary inward (Signal, then Nexus, then SEMA). In
  `testing-trace` builds, those lifecycle hook calls emit generated object
  names (`SemaStarted`, `NexusStarted`, `SignalStarted`, then the matching
  stopped names), proving the lifecycle path is live without introducing full
  actor mailbox or backpressure machinery.
- Nexus mail lifecycle state is represented with generated schema nouns:
  `MailLedgerEvent`, `SentMail`, `ProcessedMail`, and `OriginRoute`. The route
  is minted separately from the message identifier and carried in the Signal,
  Nexus, and SEMA root envelopes, the sent event, and the processed event so
  the reply has the same return address as the accepted signal input.
- Async mail flow is implemented as object flow. `Engine` owns Nexus behavior,
  `SignalActor` owns Signal admission, `MailLedger` owns lifecycle hook
  recording, `Store` owns SEMA behavior, and generated schema nouns are the
  method surfaces that move through them. The pilot should not grow free
  routing helpers beside the generated objects.
- Runtime-triad tests use schema-emitted data types as their witnesses:
  `MailLedgerEvent` for lifecycle hooks, `NexusWork`/`NexusAction` for Nexus
  execution, and split SEMA write/read roots for SEMA operations. Test-only
  enums are not valid substitutes for the schema objects whose path is being
  proved.
- Optional testing instrumentation emits structured trace events from live
  runtime calls, not source-text scans. The `testing-trace` surface observes
  Signal admission/reply, Nexus execution/decision, SEMA write application,
  and SEMA read observation while preserving default production binary
  behavior.
- Trace transport mechanics come from `triad-runtime` in trace builds. Spirit
  owns the generated `TraceEvent` object and actor hook emission; the shared
  runtime owns the generic in-memory log, length-prefixed binary trace frame,
  and Unix trace socket listener. Backpressure and deeper runtime-control
  machinery are deferred future work, not part of this production slice.
- The testing trace surface is live across the real daemon boundary. A
  trace-enabled daemon can write rkyv-encoded `TraceEvent` frames to a typed
  Unix trace socket named in binary `Configuration`; a trace-enabled CLI uses
  the shared `triad-runtime` generic trace client to bind that socket through
  `SPIRIT_TRACE_SOCKET`, send the normal binary request on the normal
  socket, decode trace frames as typed `TraceEvent` values, and print
  generated NOTA trace lines only at the display edge after the normal Signal
  reply. The normal daemon/CLI packages do not enable this surface.
- Trace events are emitted through hooks on the schema-generated
  `SignalEngine`, `NexusEngine`, and `SemaEngine` traits, not through ad-hoc
  source grep, detached helper functions, or parallel local trace traits. The
  generated traits provide default no-op trace hooks and wrapper methods; the
  runtime actors override those hooks in trace builds. The runtime proof must
  exercise the actual CLI -> daemon -> Signal -> Nexus -> SEMA -> Signal path.
- Trace events carry a schema-generated typed `ObjectName`, not a free string.
  The generated trait wrappers know plane-local actor objects such as
  `SignalObjectName::Triaged`, `NexusObjectName::Entered`,
  `SemaObjectName::WriteApplied`, and `SemaObjectName::ReadObserved`;
  trace-enabled actors override one activation hook per plane and record those
  objects through the shared `ObjectName` wrapper. `TraceEvent` is a
  transparent generated newtype over that object name, so the CLI/reporting
  edge renders one generated NOTA object such as `(Sema WriteApplied)`, not a
  double-delimited one-field wrapper. Tests assert the runtime events crossed Signal
  admission, `SignalEngine`, `NexusEngine`, and `SemaEngine`, so the witness
  proves actor/interface use instead of source-string presence. In
  `testing-trace` builds, `Engine::new` installs a shared recording trace log
  by default; callers only choose a different destination when they need a
  socket or an explicit disabled sink.
- Client-side trace collection is generic runtime behavior, not CLI-local
  component logic. The CLI supplies the component-specific trace socket
  environment variable and uses the shared typed trace client; future
  `schema-rust-next` emission should remove the remaining component-specific
  `TraceEventFrame` and NOTA display adapter. The same client surface can hand
  typed events to a future trace/introspect SEMA store instead of printing
  them; the CLI should remain a thin wrapper around that reusable client
  behavior.
- The flake exposes separate normal and trace-enabled packages. The default
  package remains the lean normal CLI + daemon pair; `packages.trace`,
  `packages.trace-cli`, and `packages.trace-daemon` build the testing-trace
  surface explicitly.
- A future "last version" package is intended for upgrade/switchover testing
  and old-client compatibility, but it must point at a real previous release
  input/tag. This repository currently has no release tag wired as a previous
  version, so the package is not faked as an alias to current main.
- The store is the SEMA writer. SEMA means database work: real SEMA writes
  durable state to the component database file. Runtime state changes pass
  through the generated `SemaEngine::apply(sema::Sema<sema::WriteInput>) ->
  sema::Sema<sema::WriteOutput>` surface, while reads pass through
  `SemaEngine::observe(sema::Sema<sema::ReadInput>) ->
  sema::Sema<sema::ReadOutput>`.
- Spirit-next tracks production Spirit 0.3 behavior where the schema surface
  reaches it while also making the read interface more developed than a toy
  single-variant enum: entries carry multiple topics, `Observe(Query)` supports
  generated `TopicMatch::{Partial,Full}` plus optional kind and generated
  `PrivacySelection` filters, `Lookup(RecordIdentifier)` returns
  `Output::RecordFound`, `Count(Query)` returns `Output::RecordsCounted`,
  observations return a multi-entry `RecordSet`, and
  `Remove(RecordIdentifier)` is a database-work operation that returns
  generated `Output::RecordRemoved`. Privacy is a directional generated
  `Magnitude`: `Zero` is open/public, and higher values narrow the intended
  audience.
- SEMA is durable (records 1007/1008, bead `primary-q2au`). `Store` maps
  generated SEMA roots onto sema-engine identified-record operations over a
  `*.sema` file: each `Record` calls `Engine::assert_identified`, each
  `Remove` calls `Engine::retract_identified`, and `Observe`, `Lookup`, and
  `Count` read through `Engine::match_identified`. sema-engine owns the
  database handle, numeric record identifier allocation, durable
  `CommitSequence`, and commit log; Spirit owns only the schema-specific SEMA
  mapping and query predicate. The file extension is `.sema` (not `.redb`) so
  the name states the runtime plane, not the implementation library.
- Production-candidate handover is tested as a copy of a real `.sema` file.
  The candidate daemon must read state seeded by the production-like daemon,
  resume the copied commit ledger, and write only to the copied database; a
  reopened production database must not see candidate writes.
- SEMA replies carry generated `DatabaseMarker` values so Signal replies can
  report the state commit sequence and digest that accepted or observed the
  request. `StateDigest` is a real content-addressed hash (blake3 over the
  committed records, reduced to the schema's `Integer` width), not a toy fold;
  an empty store digests to zero.
- The daemon's single argument is a path to a binary rkyv `Configuration`
  object, not a NOTA configuration string. The `.sema` database path fills a
  typed binary configuration field, with no flags. Text-facing launchers or
  tests may create that file, but the daemon startup path only decodes binary
  state.
- Future daemon configuration should arrive as typed binary signal surfaces,
  not by linking a NOTA decoder into the daemon. A daemon may expose multiple
  signal protocols/interfaces; configuration is another typed signal surface
  differentiated inside the root message enumerator.
- Raw NOTA text sent to the daemon's binary socket is invalid data. Tests send
  length-prefixed NOTA bytes and arbitrary bytes through the same transport
  reader the daemon uses; the generated binary frame decoder must reject them.
- The old signal macro path is not used.
- The daemon/CLI implementation is a shim around generated interfaces until
  schema diff/upgrade and the final repo-triad split land. Durable SEMA storage
  is now implemented; schema diff/upgrade and the triad split remain.
