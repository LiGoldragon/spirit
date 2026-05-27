# spirit-next

Runnable schema-derived Spirit pilot.

`spirit-next` proves the first practical version of the new architecture:

```text
schema/lib.schema
  -> schema-next Asschema
  -> schema-rust-next checked-in generated Rust at src/schema/lib.rs
  -> CLI NOTA input
  -> generated Signal frame (short header + rkyv)
  -> daemon Executor
  -> generated SEMA command/response
  -> generated Signal frame (short header + rkyv)
  -> CLI NOTA output
```

This is not production Spirit. It is the public pilot repo for making the
schema-created interface real at a process boundary.

`build.rs` regenerates the schema output in memory and fails the build if the
checked-in `src/schema/lib.rs` is stale. The runtime imports the checked-in
module directly; `OUT_DIR` is not the source of interface truth.

## Run

Start a daemon with a single NOTA argument containing the socket path:

```sh
spirit-next-daemon "[/tmp/spirit-next.sock]"
```

Call it from the CLI:

```sh
SPIRIT_NEXT_SOCKET=/tmp/spirit-next.sock \
  spirit-next "(Record ([schema] Constraint [schema creates the interface] Maximum))"
```

The CLI accepts NOTA. The daemon socket carries length-prefixed rkyv bytes
with an 8-byte short header.

## Runtime triad

`spirit-next` is the implementation target for the refined runtime triad:

- Signal is generated `Input`/`Output` plus the generated route/header/rkyv
  frame methods.
- Executor is `Engine::handle`, which lowers `Input` to `SemaCommand` and
  maps `SemaResponse` back to `Output`.
- SEMA is `Store::apply(SemaCommand)`, currently in-memory and deliberately
  isolated as the single write path.

## Local schema stack check

When editing `nota-next`, `schema-next`, or `schema-rust-next` together with
this consumer, run the local override check:

```sh
scripts/check-local-schema-stack
```

It runs `nix flake check` while overriding the schema-stack source inputs to
the latest local checkouts under `/git/github.com/LiGoldragon/`. Override those
paths with `NOTA_NEXT_PATH`, `SCHEMA_NEXT_PATH`, and
`SCHEMA_RUST_NEXT_PATH` when testing a different checkout.
