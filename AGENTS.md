# spirit agent notes

Read this repo's `ARCHITECTURE.md` and `README.md` before editing code.

`spirit` is the production Spirit daemon and the exemplar schema-derived triad
runtime. The ordinary public contract lives in `signal-spirit`; the owner-only
meta contract lives in `meta-signal-spirit`; this repo owns the daemon-local
Nexus, SEMA, and daemon runtime schema surfaces.

Load-bearing rules for this repo:

- Keep the daemon binary on the one binary rkyv configuration argument; NOTA is
  a CLI/text-edge feature, not daemon startup parsing.
- Do not hand-edit generated runtime modules as the effective fix. Edit
  `schema/*.schema` and regenerate with `SPIRIT_UPDATE_SCHEMA_ARTIFACTS=1 cargo
  build`; commit the checked-in generated `src/schema/*.rs` outputs when they
  change.
- Preserve the no-NOTA daemon dependency invariant: `nota-text` gates the text
  surface, and `tests/dependency_surface.rs` guards the binary-only build.
- Process-boundary behavior should be proven through the real daemon/CLI path,
  not only in-memory helpers.
