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
spirit-write-configuration "(ConfigurationWriteRequest /tmp/spirit.sock (Some /tmp/spirit-meta.sock) /tmp/spirit.sema None None /tmp/spirit.config.rkyv)"
spirit-daemon /tmp/spirit.config.rkyv
```

Call it from the CLI:

```sh
SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Record (([(Software (Data SchemaEvolution))] Constraint [schema creates the interface] Maximum Minimum Zero []) ([schema creates the interface] None)))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Observe ((Full [(Software (Data SchemaEvolution))]) Any Any Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Observe ((Full [(Software (Data SchemaEvolution))]) Any Any Any (Some Constraint) (Exact Zero) (ExactCertainty Zero) Any))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Observe ((Full [(Software (Data SchemaEvolution))]) (AllKeywords [schema]) (ContainsText interface) Any (Some Constraint) (Exact Zero) (AtLeastCertainty Minimum) Any))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Remove (1 ([remove obsolete record] None)))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit Version
```

The CLI accepts NOTA. The daemon socket carries length-prefixed rkyv bytes
with an 8-byte short header.

Entries carry a vector of domains. Queries use generated `DomainMatch` values:
`(Partial [(Software (Data SchemaEvolution)) (Information Documentation)])` matches any requested
domain, while `(Full [(Software (Data SchemaEvolution)) (Information Documentation)])` requires
every requested domain. The query kind is optional: `(Some Decision)` filters
by kind and `None` searches without a kind predicate. The full generated query
carries domain, keyword, text, referent, kind, privacy, certainty, and
importance predicates. `KeywordMatch` reads
asterisk-marked description spans such as `*schema language*`; `TextMatch` is a
case-insensitive full-text substring fallback. `ReferentSelection` filters by
registered runtime referents; aliases are canonicalized through
`RegisterReferent`. Ordinary `Observe` and `Count` should use
`(AtLeastCertainty Minimum)` to hide zero-certainty removal candidates; use
`(ExactCertainty Zero)` when reviewing candidates. Certainty and importance are
separate stored axes: certainty names confidence/currentness, while importance
names intrinsic significance and reaffirmation strength.

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
