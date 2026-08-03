# spirit

`spirit` is the durable intent service. Spirit 0.26 carries storage schema 14
and the revision-2 signal contract.

An active `Entry` has exactly four fields:

```text
Entry { Domains Kind Description Importance }
```

Every active record is visible through the same read plane. There are no
secondary record classes or implicit cleanup candidates. Removal is expressed
through the explicit lifecycle operations `Retire`, `Supersede`, and
`ResolveClarification`; retired substance is copied to the lifecycle archive
before the live row is retracted.

## Runtime

The authored schemas generate the checked-in Rust contracts:

```text
schema/{nexus,sema}.schema + producer schemas
  -> build-time canonicalization and Rust generation
  -> src/schema/{nexus,sema,daemon}.rs
  -> Signal admission
  -> Nexus decisions
  -> SEMA reads and writes
  -> Signal reply
```

`build.rs` fails when a generated artifact is stale. CLI input and output use
NOTA. Daemon sockets carry length-prefixed revision-2 signal frames with rkyv
payloads. The working socket accepts ordinary operations; the owner-only meta
socket carries configuration, import, and recovery controls.

Create a binary daemon configuration and start a local instance:

```sh
spirit-write-configuration \
  "(ConfigurationWriteRequest (/tmp/spirit.sock (Some /tmp/spirit-meta.sock) /tmp/spirit.sema None Gating None /tmp/spirit.config.rkyv))"
spirit-daemon /tmp/spirit.config.rkyv
```

Then use the working CLI:

```sh
SPIRIT_SOCKET=/tmp/spirit.sock spirit \
  "(Record (([(Technology (Software (Data SchemaEvolution)))] Constraint [schema creates the interface] Medium) ([([schema creates the interface] None)] [schema creates the interface])))"

SPIRIT_SOCKET=/tmp/spirit.sock spirit \
  "(Observe ((Full [(Technology (Software (Data SchemaEvolution)))]) Any Any (Some Constraint) Any))"

SPIRIT_SOCKET=/tmp/spirit.sock spirit \
  "(TextSearch [schema interface])"

SPIRIT_SOCKET=/tmp/spirit.sock spirit Version
```

Queries contain domain, keyword, description-text, kind, and importance
predicates. `TextSearch` is the compact ranked lookup over active record
descriptions. `Intent` reads active records under typed domain scopes. Both
return the same `ObservedRecord` shape as `Observe`.

## Admission and lifecycle

When required, the daemon sends one typed `JudgeAdmission` request to the
external Spirit judge for each proposed write. Its packet contains the typed
operation, relevant existing-record context, and the database marker. The
guardian either accepts or returns a typed rejection; unavailable, malformed,
or timed-out judgment fails closed. Decisions are audit state in the separate
schema-7 guardian journal.

Lifecycle operations preserve retained fields:

- `ChangeRecord` replaces one live four-field entry under the same identifier.
- `Retire` archives the live entry and retracts it.
- `Supersede` archives and retracts its targets, then records replacements.
- `ResolveClarification` edits its targets and archives/retracts the standalone
  clarification.

## Storage schema 14 migration

`spirit-migrate-store` is an offline, one-way v13-to-v14 projection. Stop the
daemon and operate on the intended state path only. The migration:

1. copies the exact quiesced v13 live store, optional archive, and optional
   schema-6 guardian journal into a mode-`0700` rollback directory;
2. validates every v13 family through the frozen migration-only reader;
3. projects only identifier, domains, kind, description, and importance;
4. builds fresh live and archive schema-14 stores and reopens both for
   comparison before exposure;
5. exposes the archive first and live store second;
6. starts fresh schema-14 history with projected assertions and one migration
   receipt; the current guardian journal starts at schema 7.

The rollback directory is `<live-stem>.schema-13-rollback`. Its copies are the
only supported recovery material for the discarded v13 representation. A
second successful invocation reports `Current` without modifying the v14
store. Corrupt or wrong-generation input is rejected without replacing it.

The old decoder exists only behind the `production-migration` feature in
`production_migration::v13`; the daemon, working CLI, current schema, and
current store do not contain legacy families or compatibility tables.

## Release surface

The repository flake is the one-root authority for the complete Spirit user
service. It exports the daemon, working and meta CLIs, configuration writer,
store migration tool, judge, judge configuration, provider, release manifest,
and `lib.<system>.mkUserServiceArtifacts`.

Consumers pin Spirit once and obtain the whole service bundle from that root.
The release manifest asserts the exact producer-contract revisions used by the
daemon and judge.

## Test boundaries

Repository tests use disposable stores and isolated child-process
environments. They never open deployed Spirit state. The migration suite seeds
real schema-13 live/archive layouts, verifies byte-identical private rollback,
checks the fresh schema-14 log and receipt, and proves a second run is a no-op.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the contract and durability details.
