# INTENT — spirit-next

`spirit-next` exists to prove a running Spirit-like component can be built
from schema-derived interfaces.

It is intentionally separate from production `spirit`/`persona-spirit` so
operators can iterate without disturbing the deployed intent substrate.

Load-bearing constraints:

- CLI input and output are NOTA.
- Component/process communication is binary rkyv.
- Rust data types are generated from `schema/spirit.schema`.
- The old signal macro path is not used.
- The daemon/CLI implementation is a shim around generated interfaces until
  schema-derived frame dispatch, executor lowering, and SEMA response shaping
  land.
