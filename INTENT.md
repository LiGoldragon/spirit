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
- Schema lowering goes through `schema-next`'s macro registry before Rust
  emission; the build must fail if the registry does not reach nested
  struct-field and enum-variant type bodies.
- `build.rs` is a freshness witness for the generated source. It regenerates
  in memory and fails if `src/schema/lib.rs` is missing or stale.
- The schema declares the runtime triad surfaces:
  `Input`/`Output` for Signal and `SemaCommand`/`SemaResponse` for
  state work.
- The executor lowers generated Signal input into generated SEMA command,
  then turns generated SEMA response into generated Signal output.
- The store is the SEMA writer. Runtime state changes pass through
  `Store::apply(SemaCommand)` rather than direct mutation from the Signal
  layer.
- The old signal macro path is not used.
- The daemon/CLI implementation is a shim around generated interfaces until
  durable redb storage, schema diff/upgrade, and the final repo-triad split
  land.
