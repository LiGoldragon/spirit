# spirit

`spirit` is the production Spirit daemon and the first practical version of the
new schema-derived architecture:

```text
schema/{signal,nexus,sema}.schema
  -> schema-next SchemaSource typed source objects
  -> rkyv-serializable schema-in-Rust values
  -> schema-rust-next checked-in generated Rust at src/schema/{signal,nexus,sema}.rs
  -> CLI NOTA input
  -> generated Signal frame (short header + rkyv)
  -> daemon SignalActor
  -> Nexus mail keeper / translator
  -> generated SEMA input/output against Store
  -> Nexus reply translation
  -> generated Signal frame (short header + rkyv)
  -> CLI NOTA output
```

It is also the copyable triad exemplar for newer components: authored schema
source generates the Signal/Nexus/SEMA nouns, the CLI is a text edge, daemon
traffic is binary rkyv, and the durable store is owned by SEMA.

`build.rs` decodes `schema/{signal,nexus,sema}.schema` into typed
`SchemaSource` values, validates canonical text and rkyv round-trips, emits
Rust from those typed values, and fails the build if any generated
`src/schema/*.rs` plane module is stale. The runtime imports the checked-in
module directly; `OUT_DIR` is not part of the runtime schema surface.

## Run

Create a startup archive with the text-edge writer, then start the daemon with
one argument: the path to the binary rkyv `SpiritDaemonConfiguration` file. The
configuration carries the Unix socket path, required meta socket path, and
`.sema` database path. The daemon does not parse NOTA at startup.

```sh
spirit-write-configuration "(ConfigurationWriteRequest /tmp/spirit.sock (Some /tmp/spirit-meta.sock) /tmp/spirit.sema None /tmp/spirit.config.rkyv)"
spirit-daemon /tmp/spirit.config.rkyv
```

Call it from the CLI:

```sh
SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Record ([schema] Constraint [schema creates the interface] Maximum Minimum Zero))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Observe ((Full [schema]) (Some Constraint) (Exact Zero)))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Observe ((Full [schema]) (Some Constraint) (Exact Zero) (ExactCertainty Zero)))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Remove 1)"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit Version
```

The CLI accepts NOTA. The daemon socket carries length-prefixed rkyv bytes
with an 8-byte short header.

Entries carry a vector of topics. Queries use generated `TopicMatch` values:
`(Partial [schema runtime])` matches any requested topic, while
`(Full [schema runtime])` requires every requested topic. The query kind is
optional: `(Some Decision)` filters by kind and `None` searches only by topic.
The full generated query also carries privacy, certainty, and weight selectors.
The CLI accepts the common three-field query shorthand and fills the certainty
selector with `AtLeastCertainty Minimum` and the weight selector with `Any`, so
ordinary `Observe` and `Count` hide zero-certainty removal candidates. Use the
explicit four-field form with `ExactCertainty Zero` when reviewing candidates,
or the five-field form to add a `WeightSelection`. Certainty and weight are
separate stored axes: certainty names confidence/currentness, while weight names
importance/repetition and is used for filtering and high-weight-first retrieval.

## Runtime triad

`spirit` is the implementation target for the refined runtime triad:

- Signal is generated `Input`/`Output` plus the generated route/header/rkyv
  frame methods.
- Nexus is the decision keeper and translator. It accepts schema-emitted Signal
  mail, lowers it into generated SEMA write/read input, holds the origin route
  while SEMA runs, and maps the generated SEMA output back to generated Signal
  output.
- SEMA is split by generated traits: `Store::apply` takes mutable write input,
  while `Store::observe` takes shared read input. Both operate over the durable
  `.sema` component database through `sema-engine`.

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
