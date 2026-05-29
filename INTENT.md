# INTENT — spirit-next

`spirit-next` exists to prove a running Spirit-like component can be built
from schema-derived interfaces.

It is intentionally separate from production `spirit`/`persona-spirit` so
operators can iterate without disturbing the deployed intent substrate.

Load-bearing constraints:

- CLI input and output are NOTA.
- Component/process communication is binary rkyv.
- Rust data types are generated from the crate-local `schema/lib.schema`
  entrypoint and materialized as checked-in source under `src/schema/`.
- `schema/lib.schema` currently uses the transitional pipe-family declaration
  syntax accepted by schema-next. The target authored syntax is now the
  name-first `@` form (`Name@{...}`, `Name@(...)`, `name@Type`), while the
  generated `Asschema` and emitted Rust should stay equivalent. Future
  syntax-migration work edits the schema source first and then regenerates
  `src/schema/lib.rs`.
- The generated file path from schema-rust is crate-relative
  `src/schema/lib.rs`; build code uses that path directly instead of treating
  `schema/lib.rs` as relative to `src/`.
- Schema lowering goes through `schema-next` before Rust emission; build and
  runtime tests prove the generated `Asschema` data and emitted Rust, not a
  macro trace side channel.
- `build.rs` is a freshness witness for the generated source. It regenerates
  in memory and fails if `src/schema/lib.rs` is missing or stale.
- The schema declares the runtime triad surfaces:
  `Input`/`Output` for Signal, `NexusInput`/`NexusOutput` for execution mail,
  and `SemaInput`/`SemaOutput` for database work.
- Generated plane namespaces expose the same shapes as `signal::Input` /
  `signal::Output`, `nexus::Input` / `nexus::Output`, and `sema::Input` /
  `sema::Output`. The flat names are bootstrap backing names; runtime trait
  signatures and tests use the plane namespaces so the plane carries the
  ancestry instead of every payload name.
- Cross-plane branching uses generated `schema::Plane::{Signal,Nexus,Sema}`.
  Each variant carries the actual plane envelope; there is no parallel
  `Kind` tag that must be paired with a separate message body.
- Each language plane has input/output and reusable import/export vocabulary.
  Import/export paths mirror Rust module namespaces with a single colon rather
  than double colon, for example `signal:sema:Magnitude`.
- Signal, Nexus, and SEMA use the same authored schema shape: imports/exports,
  roots, and namespace. Their generated Rust differs by trait support and
  runtime ownership, not by a separate notation.
- Nexus is the execution and mail-keeper plane between Signal and SEMA. Signal
  input becomes `NexusMail<Payload>` with a `MessageIdentifier` and an
  auto-created `OriginRoute`; while Nexus owns that object, the mail is being
  processed.
- Nexus is a real runtime object (`Nexus`), not a set of methods on the
  orchestrator. It OWNS the durable SEMA store handle and the mail ledger, and
  holds the mail in a TYPE-LEVEL being-processed state across the SEMA call:
  the in-flight mail is `Mail<BeingProcessed>` and only running SEMA produces
  `Mail<Processed>`. "Nexus holds the mail ⇒ it is being processed" is a
  compile-time fact, not a log entry (record 970). `Engine` is a thin composer
  of the three centers — Signal admission, the Nexus mail keeper, and the SEMA
  store it owns — and never calls the store directly; the SEMA invocation lives
  inside `Nexus::process`, between the inbound and outbound translations Nexus
  owns.
- Signal admission is explicit in the pilot. `SignalActor::accept` validates
  the generated `Input` enough to create `SignalAccepted`, and
  `SignalAccepted::process_with` fires the generated `MessageSent` hook before
  the object enters Nexus.
- Signal rejection is also schema-emitted. Invalid Signal input returns
  `Output::Rejected(SignalRejection { validation_error, database_marker })`
  where `ValidationError` is generated from `schema/lib.schema`; the runtime
  does not use a hand-written rejection enum at the wire boundary.
- Nexus lowers generated Signal payload mail into generated `NexusInput`, then
  emits generated `NexusOutput::Sema(SemaInput)`. When SEMA replies with
  `SemaOutput`, Nexus turns it into generated Signal output and records
  `MessageProcessed<Output>`.
- `schema-rust-next` emits `NexusEngine` and `SemaEngine` when the schema
  declares the corresponding input/output pairs. `Engine` implements
  `NexusEngine`; `Store` implements `SemaEngine`; tests call those trait
  surfaces with generated schema objects.
- Nexus mail lifecycle state is represented with generated schema nouns:
  `MailLedgerEvent`, `SentMail`, `ProcessedMail`, and `OriginRoute`. The route
  is minted separately from the message identifier and carried in the Signal,
  Nexus, and SEMA root envelopes, the sent event, the in-flight Nexus mail, and
  the processed event so the reply has the same return address as the accepted
  signal input.
- Async mail flow is implemented as object flow. `Engine` owns Nexus behavior,
  `SignalActor` owns Signal admission, `MailLedger` owns lifecycle hook
  recording, `Store` owns SEMA behavior, and generated schema nouns are the
  method surfaces that move through them. The pilot should not grow free
  routing helpers beside the generated objects.
- Runtime-triad tests use schema-emitted data types as their witnesses:
  `MailLedgerEvent` for lifecycle hooks, `NexusInput`/`NexusOutput` for Nexus
  execution, and `SemaInput`/`SemaOutput` for SEMA operations. Test-only enums
  are not valid substitutes for the schema objects whose path is being proved.
- The store is the SEMA writer. SEMA means database work: real SEMA writes
  durable state to the component database file. Runtime state changes pass
  through the generated `SemaEngine::apply(sema::Sema<sema::Input>) ->
  sema::Sema<sema::Output>` surface rather than direct mutation from the Signal
  layer.
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
- SEMA replies carry generated `DatabaseMarker` values so Signal replies can
  report the state commit sequence and digest that accepted or observed the
  request. `StateDigest` is a real content-addressed hash (blake3 over the
  committed records, reduced to the schema's `Integer` width), not a toy fold;
  an empty store digests to zero.
- The daemon's single NOTA argument is a positional `(socket database)` record:
  the `.sema` database path fills an existing configuration position, with no
  flag (the single-argument rule holds). A bare socket-path argument still
  works, defaulting the `.sema` file beside the socket.
- The old signal macro path is not used.
- The daemon/CLI implementation is a shim around generated interfaces until
  schema diff/upgrade and the final repo-triad split land. Durable SEMA storage
  is now implemented; schema diff/upgrade and the triad split remain.
