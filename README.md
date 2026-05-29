# spirit-next

Runnable schema-derived Spirit pilot.

`spirit-next` proves the first practical version of the new architecture:

```text
schema/lib.schema
  -> schema-next Asschema
  -> schema-rust-next checked-in generated Rust at src/schema/lib.rs
  -> CLI NOTA input
  -> generated Signal frame (short header + rkyv)
  -> daemon SignalActor
  -> Nexus mail keeper / translator
  -> generated SEMA input/output against Store
  -> Nexus reply translation
  -> generated Signal frame (short header + rkyv)
  -> CLI NOTA output
```

This is not production Spirit. It is the public pilot repo for making the
schema-created interface real at a process boundary.

`build.rs` regenerates the schema output in memory and fails the build if the
checked-in `src/schema/lib.rs` is stale. The runtime imports the checked-in
module directly; `OUT_DIR` is not the source of interface truth.

## Run

Start a daemon with a single NOTA argument containing the socket path. A two
field positional record can provide both socket and database paths:

```sh
spirit-next-daemon "[/tmp/spirit-next.sock]"
# or
spirit-next-daemon "([/tmp/spirit-next.sock] [/tmp/spirit-next.sema])"
```

Call it from the CLI:

```sh
SPIRIT_NEXT_SOCKET=/tmp/spirit-next.sock \
  spirit-next "(Record ([[schema]] Constraint [schema creates the interface] Maximum))"

SPIRIT_NEXT_SOCKET=/tmp/spirit-next.sock \
  spirit-next "(Observe ((Full [[schema]]) (Some Constraint)))"

SPIRIT_NEXT_SOCKET=/tmp/spirit-next.sock \
  spirit-next "(Remove 1)"
```

The CLI accepts NOTA. The daemon socket carries length-prefixed rkyv bytes
with an 8-byte short header.

Entries carry a vector of topics. Queries use generated `TopicMatch` values:
`(Partial [[schema] [runtime]])` matches any requested topic, while
`(Full [[schema] [runtime]])` requires every requested topic. The query kind is
optional: `(Some Decision)` filters by kind and `None` searches only by topic.

## Runtime triad

`spirit-next` is the implementation target for the refined runtime triad:

- Signal is generated `Input`/`Output` plus the generated route/header/rkyv
  frame methods.
- Nexus is the mail keeper and translator. It accepts schema-emitted Signal
  mail, lowers it into generated SEMA input, holds the origin route while SEMA
  runs, and maps the generated SEMA output back to generated Signal output.
- SEMA is `Store::apply(sema::Sema<sema::Input>)`, a durable redb writer over
  the `.sema` database file and the single state mutation path.

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
