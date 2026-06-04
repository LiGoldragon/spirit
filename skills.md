# Skills — spirit

Read the workspace Rust and component skills before editing this repo:

- `skills/component-triad.md`
- `skills/rust-discipline.md`
- `skills/rust/methods.md`
- `skills/rust/errors.md`
- `skills/rust/storage-and-wire.md`
- `skills/rust/crate-layout.md`
- `skills/abstractions.md`
- `skills/actor-systems.md`

Schema-generated nouns are the method surface. Runtime behavior belongs on
`SignalActor`, `Nexus`, `Store`, generated root types, or shared runtime
objects such as `triad-runtime` trace types; do not add helper modules that
bypass the generated Signal/Nexus/SEMA traits.
