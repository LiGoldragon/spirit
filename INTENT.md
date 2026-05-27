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
- The generated file path from schema-rust is crate-relative
  `src/schema/lib.rs`; build code uses that path directly instead of treating
  `schema/lib.rs` as relative to `src/`.
- Schema lowering goes through `schema-next`'s macro registry before Rust
  emission; the build must fail if the registry does not reach nested
  struct-field and enum-variant type bodies.
- `build.rs` is a freshness witness for the generated source. It regenerates
  in memory and fails if `src/schema/lib.rs` is missing or stale.
- The schema declares the runtime triad surfaces:
  `Input`/`Output` for Signal, `NexusInput`/`NexusOutput` for execution mail,
  and `SemaInput`/`SemaOutput` for state work.
- Each language plane has input/output and reusable import/export vocabulary.
  Import/export paths mirror Rust module namespaces with a single colon rather
  than double colon, for example `signal:sema:Magnitude`.
- Nexus is the execution and mail-keeper plane between Signal and SEMA. Signal
  input becomes `NexusMail<Payload>` with a `MessageIdentifier`; while Nexus
  owns that object, the mail is being processed.
- Nexus lowers generated Signal payload mail into generated `NexusInput`, then
  emits generated `NexusOutput::Sema(SemaInput)`. When SEMA replies with
  `SemaOutput`, Nexus turns it into generated Signal output and records
  `MessageProcessed<Output>`.
- Nexus mail lifecycle state is represented with generated schema nouns:
  `MailLedgerEvent`, `SentMail`, and `ProcessedMail`.
- The store is the SEMA writer. Runtime state changes pass through
  `Store::apply(SemaInput)` rather than direct mutation from the Signal
  layer.
- SEMA replies carry generated `DatabaseMarker` values so Signal replies can
  report the state commit sequence and digest that accepted or observed the
  request.
- The old signal macro path is not used.
- The daemon/CLI implementation is a shim around generated interfaces until
  durable redb storage, schema diff/upgrade, and the final repo-triad split
  land.
