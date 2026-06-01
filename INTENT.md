# INTENT — spirit-next

`spirit-next` exists to prove a running Spirit-like component can be built
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
  of `schema/lib.schema`, emits Rust from the checked-in `.asschema` artifact,
  and keeps `.asschema.rkyv` as a generated binary cache/witness.
- `schema/lib.schema` preserves NOTA brace semantics: braces are key-value
  maps, not collections of one-object declarations. A namespace entry is a
  pair such as `Topic String`, `Entry { Topics * Kind * }`, or `Kind [...]`.
  Struct fields are also key-value pairs; `Topics *` means the key names the
  field and `*` reuses the same type, while `kind (Optional Kind)` binds a
  field to a different composite reference. Enum bodies are square-bracket
  variant lists whose elements are one type: bare PascalCase symbols for unit
  variants and parenthesized records such as `(Record Entry)` for
  data-carrying variants.
  Single-reference declarations such as `Topic String` and
  `Topics (Vec Topic)` lower to real tuple newtypes, not one-field maps. The
  generated `Asschema` and emitted Rust consume only the strict authored
  surface.
- The generated file path from schema-rust is crate-relative
  `src/schema/lib.rs`; build code uses that path directly instead of treating
  `schema/lib.rs` as relative to `src/`.
- Schema lowering goes through `schema-next` before Rust emission; build and
  runtime tests prove the generated `Asschema` data and emitted Rust, not a
  macro trace side channel. The build freshness path materializes
  `AsschemaArtifact` as legal `.asschema` NOTA and `.asschema.rkyv` files,
  compares the generated NOTA artifact with checked-in `schema/lib.asschema`,
  then emits Rust from that checked-in artifact path. The checked-in generated
  Rust is produced from serialized assembled-schema data rather than a private
  lowerer-to-emitter value.
- `build.rs` is a freshness witness for the generated source. It regenerates
  in memory and fails if `src/schema/lib.rs` is missing or stale.
- Old design-convenience APIs do not remain beside the working interface
  (Spirit record 1339). Once the schema-derived trait path exists, parallel
  bypass/convenience surfaces are removed rather than carried for comfort.
- The schema declares the runtime triad surfaces:
  `Input`/`Output` for Signal, `NexusInput`/`NexusOutput` for execution mail,
  `SemaWriteInput`/`SemaWriteOutput` for database mutations, and
  `SemaReadInput`/`SemaReadOutput` for database reads.
- Generated plane namespaces expose the same shapes as `signal::Input` /
  `signal::Output`, `nexus::Input` / `nexus::Output`,
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
  a generated `nexus::Nexus<nexus::Input>` envelope directly; that envelope is
  the only Signal-to-Nexus runtime handoff.
- Nexus is a real runtime object (`Nexus`), not a set of methods on the
  orchestrator. It owns the durable SEMA store handle and the mail ledger. The
  mutable `NexusEngine::execute(&mut self, nexus::Nexus<nexus::Input>)`
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
- Nexus lowers generated Signal input into generated `NexusOutput::SemaWrite`
  or `NexusOutput::SemaRead`. Write operations call `SemaEngine::apply`; read
  operations call `SemaEngine::observe`. When SEMA replies with
  `SemaWriteOutput` or `SemaReadOutput`, Nexus turns the reply into generated
  Signal output and records `MessageProcessed<Output>`.
- `schema-rust-next` emits `SignalEngine`, `NexusEngine`, and `SemaEngine`
  when the schema declares the corresponding input/output pairs. `SignalActor`
  implements `SignalEngine` as the lightweight triage/reply boundary; `Nexus`
  implements mutable `NexusEngine` as the computation and decision plane;
  `Store` implements `SemaEngine` as the durable state plane with split
  `apply(&mut self, WriteInput)` and `observe(&self, ReadInput)` surfaces;
  tests call those trait surfaces with generated schema root objects.
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
  `MailLedgerEvent` for lifecycle hooks, `NexusInput`/`NexusOutput` for Nexus
  execution, and split SEMA write/read roots for SEMA operations. Test-only
  enums are not valid substitutes for the schema objects whose path is being
  proved.
- Optional testing instrumentation emits structured trace events from live
  runtime calls, not source-text scans. The `testing-trace` surface observes
  Signal admission/reply, Nexus execution/decision, SEMA write application,
  and SEMA read observation while preserving default production binary
  behavior.
- The testing trace surface is live across the real daemon boundary. A
  trace-enabled daemon can write rkyv-encoded `TraceEvent` frames to a typed
  Unix trace socket named in binary `Configuration`; a trace-enabled CLI can
  bind that socket through `SPIRIT_NEXT_TRACE_SOCKET`, send the normal binary
  request on the normal socket, decode trace frames, and print human-facing
  trace lines after the normal Signal reply. The normal daemon/CLI packages do
  not enable this surface.
- Trace events are emitted through hooks on the schema-generated
  `SignalEngine`, `NexusEngine`, and `SemaEngine` traits, not through ad-hoc
  source grep, detached helper functions, or parallel local trace traits. The
  generated traits provide default no-op trace hooks and wrapper methods; the
  runtime actors override those hooks in trace builds. The runtime proof must
  exercise the actual CLI -> daemon -> Signal -> Nexus -> SEMA -> Signal path.
- Trace events carry actor/interface semantics. Tests assert that runtime
  events crossed Signal admission, `SignalEngine`, `NexusEngine`, and
  `SemaEngine`, so the witness proves actor/interface use instead of string
  presence. In `testing-trace` builds, `Engine::new` installs a shared
  recording trace log by default; callers only choose a different destination
  when they need a socket or an explicit disabled sink.
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
  reaches it: entries carry multiple topics, observe supports generated
  `TopicMatch::{Partial,Full}` plus an optional kind filter, observations
  return a multi-entry `RecordSet`, and `Remove(RecordIdentifier)` is a
  database-work operation that returns generated `Output::RecordRemoved`.
- SEMA is durable (records 1007/1008, bead `primary-q2au`). `Store` is a real
  redb database written to a `*.sema` file: each `Record` is a redb write
  transaction persisting the rkyv-archived `Entry`, each `Remove` is a redb
  write transaction deleting an entry and advancing the commit sequence, each
  `Observe` is a redb read transaction returning every matching entry, and the
  commit sequence + next-identifier counter persist in the database so a store
  reopened from the same `.sema` path resumes where it left off. The file
  extension is `.sema` (not `.redb`) so the name states the runtime plane, not
  the implementation library.
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
