# spirit

`spirit` is the production Spirit daemon and the first practical version of the
new schema-derived architecture:

```text
schema/{signal,nexus,sema}.schema
  -> schema SchemaSource typed source objects
  -> rkyv-serializable schema-in-Rust values
  -> schema-rust checked-in generated Rust at src/schema/{signal,nexus,sema}.rs
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
configuration carries the Unix socket path, required meta socket path, `.sema`
database path, and criome authorization mode. The daemon does not parse NOTA at
startup.

```sh
spirit-write-configuration "(ConfigurationWriteRequest (/tmp/spirit.sock (Some /tmp/spirit-meta.sock) /tmp/spirit.sema None Gating None /tmp/spirit.config.rkyv))"
spirit-daemon /tmp/spirit.config.rkyv
```

Call it from the CLI:

```sh
SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Record (([(Software (Data SchemaEvolution))] Constraint [schema creates the interface] Minimum Zero) ([schema creates the interface] None)))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(PublicIntent [(Software (Data SchemaEvolution))])"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Observe ((Full [(Software (Data SchemaEvolution))]) Any Any (Some Constraint) (Exact Zero) Any))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(Observe ((Full [(Software (Data SchemaEvolution))]) (AllKeywords [schema]) (ContainsText interface) (Some Constraint) (Exact Zero) Any))"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit "(PublicTextSearch [routing protocol])"

SPIRIT_SOCKET=/tmp/spirit.sock \
  spirit Version
```

Physical deletion is an owner-only meta operation, not a working verb. The
owner archives-then-removes records matched by an explicit structural query by
issuing `CollectRemovalCandidates` over the meta socket (the same socket that
carries `Configure`/`Import`); there is no working hard-delete path.

The CLI accepts NOTA. The daemon socket carries length-prefixed rkyv bytes
with an 8-byte short header.

Entries carry a vector of domains. Queries use generated `DomainMatch` values:
`(Partial [(Software (Data SchemaEvolution)) (Information Documentation)])` matches any requested
domain, while `(Full [(Software (Data SchemaEvolution)) (Information Documentation)])` requires
every requested domain. The query kind is optional: `(Some Decision)` filters
by kind and `None` searches without a kind predicate. The full generated query
carries domain, keyword, text, kind, privacy, and importance predicates.
`KeywordMatch` reads
asterisk-marked description spans such as `*schema language*`; `TextMatch` is a
case-insensitive full-text substring fallback. `RegisterReferent` remains an
explicit registry operation, but active entries do not carry referent lists.
Importance names intrinsic significance and reaffirmation strength; accepted
records do not carry a certainty axis.

For ordinary agent lookup, prefer `PublicIntent` with schema-backed domain
scopes. It accepts a structural domain-scope vector, expands requested paths to
exact ancestor `All` scopes, dedupes shared ancestors and repeated records, and
returns ordered public `RecordsObserved` results directly. Keep
`PublicTextSearch` as a fallback/debugging text search when the domain is not
known. Use full `Observe` when you need exact domain / kind / privacy /
importance predicates or exhaustive stashed results. `PublicRecords` remains
the lower-level/admin structural selection surface.

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

## Remote schema stack check

When editing `nota`, `schema`, or `schema-rust` together with
this consumer, commit and push the participating refs, then run the override
check:

```sh
SPIRIT_STACK_REF=operator/my-feature SPIRIT_TARGET_REF=operator/my-feature \
scripts/check-local-schema-stack
```

It runs `nix flake check` against pushed `github:LiGoldragon/...` refs while
overriding the schema-stack source inputs to the same remote ref by default.
Use per-repo variables such as `NOTA_REF`, `SCHEMA_REF`, and
`SCHEMA_RUST_REF` when the stack does not share one branch or revision.
