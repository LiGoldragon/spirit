# spirit

`spirit` is the durable intent service. Spirit 0.27.0 uses storage schema 14
and revision-2 Signal frames. Its active-record noun is exactly:

```text
Entry { Domains Kind Description Importance }
```

The query noun is exactly:

```text
Query { DomainMatch KeywordMatch TextMatch SelectedKind ImportanceSelection }
```

Justification belongs to a write request, not to `Entry`. There is no core
certainty, privacy, referent, relation, or public/private record field.

## User boundary

`spirit` and the owner-only `meta-spirit` each take exactly one inline
NOTA/DOTOS object. A bare selector such as `Version`, `Marker`, `ObserveHead`,
or `ObserveHeadObject` is already an object. File paths are not input
indirection, and flags (including `--help` and `--pretty`), zero operands, and
extra operands are invalid by design.

`SPIRIT_SOCKET` selects the ordinary socket and `SPIRIT_META_SOCKET` selects
the owner-only socket; neither adds a CLI operand. For example:

```sh
SPIRIT_SOCKET=/tmp/spirit.sock spirit Version
SPIRIT_SOCKET=/tmp/spirit.sock spirit Marker
SPIRIT_SOCKET=/tmp/spirit.sock spirit '(Count (Any Any Any None Any))'
SPIRIT_SOCKET=/tmp/spirit.sock spirit '(TextSearch [schema interface])'
SPIRIT_SOCKET=/tmp/spirit.sock spirit '(Observe (Full [(Technology (Software (Data SchemaEvolution)))]) Any Any (Some Constraint) Any)'
SPIRIT_META_SOCKET=/tmp/meta-spirit.sock meta-spirit ObserveHead
```

The exact ordinary command/type authority is
[`signal-spirit`](https://github.com/LiGoldragon/signal-spirit/tree/b37fc963292c157452d06e150296c19005dae3f2/schema);
the owner authority is
[`meta-signal-spirit`](https://github.com/LiGoldragon/meta-signal-spirit/tree/009cb6c8ddf985244189a79d554aa5d5c24605c8/schema).
See [manual.md](manual.md) for the complete current object index.

## Runtime and release

NOTA is an edge format only. The daemon receives binary revision-2 Signal
frames; its startup configuration is a private immutable binary artifact,
written by `spirit-write-configuration` and consumed by the daemon service.
It is not an exception to the public CLI grammar.

The working socket carries the 21 ordinary roots; the owner-only socket carries
`Configure`, `Import`, `ObserveHead`, and `ObserveHeadObject`. Explicit
`Retire`, `Supersede`, and `ResolveClarification` archive retained entry data
before retracting a live row.

When admission is enabled, Spirit uses the pinned external judge in the
declared production profile: OpenAI Codex `gpt-5.6-luna` with `XHigh` reasoning.
The provider session reference and all request/output content remain opaque;
diagnostics cross the contract boundary only in redacted form.

`flake.nix` is the single service release root. It pins judge
`b590c2bdd6499cc391ac01dddf2ab67b0d53bd6a`, judge configuration
`fc648d2796513b83cee27ffeb319ceb01134a60e`, and provider
`6753f8b89f173e633cdf2809bd370ac4f93c6bc0`, and exports the daemon, public
CLIs, service bundle, and release manifest.

See [ARCHITECTURE.md](ARCHITECTURE.md) for runtime and durability boundaries.
