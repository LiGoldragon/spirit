# Spirit architecture

This document describes Spirit 0.26, storage schema 14, and signal wire
revision 2.

## Contract ownership

Spirit consumes the generated Signal contract from `signal-spirit`, the meta
contract from `meta-signal-spirit`, and the guardian contract from
`signal-spirit-judge`. It authors the Nexus and SEMA schemas in this repository.
`build.rs` canonicalizes those sources, checks their rkyv representations, and
regenerates the checked-in Rust. A normal build compares generated output with
the checked-in files and fails on drift.

The current data boundary is deliberately small:

```text
Entry
  domains: Domains
  kind: Kind
  description: Description
  importance: Importance
```

That shape is the wire noun, live-store value, lifecycle-archive value,
guardian operation value, import value, and mirror payload value. Current code
does not maintain alternate entry shapes.

## Runtime triad

```text
NOTA CLI                                      owner meta CLI
   |                                               |
revision-2 Signal frame                     meta Signal frame
   |                                               |
Signal admission ---- validation                   |
   |                                               |
Nexus decision keeper <----------------------------+
   |
   +-- SEMA write: Record | BumpImportance | ChangeRecord
   |
   +-- SEMA read: Observe | Intent | TextSearch | Lookup | Count
   |
   +-- effect: admission | stash | lifecycle | subscriptions
   |
Signal reply
```

Signal owns public request validation and frame routing. Nexus owns operation
sequencing and carries the origin route across recursive work. SEMA owns
materialized database reads and writes. Effects own state outside the direct
SEMA root, including the lifecycle archive, guardian exchange, stashes, and
subscriptions.

The daemon keeps working and meta sockets distinct. Configuration is a binary
startup artifact; the daemon does not parse text configuration. NOTA is an edge
format for CLIs and configuration writers, never the daemon transport.

## Read semantics

All active records share one visibility class and one `ObservedRecord` shape.

- `Observe` applies typed domain, keyword, description-text, kind, and
  importance predicates and stashes the result for recovery.
- `Count` applies the same query without returning rows.
- `Lookup` resolves an exact record identifier.
- `Intent` retrieves records under typed domain scopes.
- `TextSearch` ranks case-insensitive description matches and caps the result.

Importance is the only magnitude-valued state on an entry. `BumpImportance`
advances it by one rung without changing the record identifier.

## Admission

When the guardian is configured or required, each candidate working write
becomes one `AdmissionJudgePacket`:

```text
AdmissionJudgePacket
  operation: Record | Propose | Clarify | ResolveClarification |
             Supersede | Retire | ChangeRecord
  records: relevant active context
  database_marker: pre-operation marker
```

The external judge returns one admission verdict and a redacted diagnostic.
Spirit maps contract rejections and transport faults into typed
`GuardianRejected` results. Required admission fails closed.

Guardian decisions are audit material, not intent material. They live in a
separate unversioned schema-7 journal whose filename includes the generation.
An older journal is left untouched and is never decoded using a newer layout.

## Explicit lifecycle

There is no implicit garbage-collection lifecycle. Each destructive semantic
change names its target:

- `ChangeRecord` replaces a live entry under the same key.
- `Retire` archives the exact live row, then retracts it.
- `Supersede` archives and retracts all targets, then records its replacement
  entries.
- `ResolveClarification` updates named targets and archives/retracts the
  standalone clarification record.

Archive-before-retract ordering is the durability invariant. The lifecycle
archive stores the same retained entry fields and original record key. It is a
separate schema-14 records store and has no migration-receipt family.

## Durable store and history

The live SEMA store has two families:

```text
RecordsFamily     key = RecordIdentifier, value = StoredRecord
MigrationsFamily  key = source generation, value = Migration
```

The storage schema generation is 14. The versioned-log identity is
`spirit:sema:v14`; store open/import and mirror shipping use that exact
identity. The generation suffix prevents checkpoints or log suffixes from
older representations from attaching to current history.

Every live mutation is represented in the sema-engine commit log. Checkpoint
plus suffix restoration must reproduce the identical materialized store and
head. Mirror envelopes carry the same generation-qualified store identity.

## Offline v13 projection

The production migration is destructive with respect to the old
representation and conservative with respect to recovery. It must run while
the daemon is stopped.

Before opening any database engine, the migration copies the exact source bytes
to `<live-stem>.schema-13-rollback`, creates the directory with mode `0700`, and
includes:

- `live.v13.sema`;
- `archive.v13.sema` when the lifecycle archive exists;
- `guardian.v6.sema` when the previous audit journal exists.

Copying precedes even read-oriented engine opens because a database open may
update internal bookkeeping. Hard links are insufficient for this guarantee.

The frozen v13 reader is compiled only with `production-migration`. It
validates the complete old family catalogue, enumerates live and archive rows,
and has no source-write API. Projection serializes and decodes each retained
field independently; it never attempts to decode an old Entry as a current
Entry.

```text
v13 live rows --------------------+
                                  +--> project retained fields
v13 archive rows -----------------+          |
                                             +--> temporary v14 live
                                             +--> temporary v14 archive
                                                     |
                                          reopen and compare both
                                                     |
                                        expose archive, then live
```

The fresh live history contains one assertion per projected active record and
one v13 migration receipt. Previous log entries, checkpoints, and migration
receipts are not replayed. The archive receives projected archive assertions
with original identifiers. The current audit journal starts fresh at schema 7;
the previous journal remains recovery material only.

Temporary stores are validated before either destination is exposed. Archive
is renamed first so a crash cannot leave a new live store paired with an old
archive. A rerun can complete from the still-authoritative source and rollback
bundle. Once the live rename succeeds, subsequent runs recognize schema 14 and
return `Current`.

Wrong-generation or corrupt sources are rejected before replacement. If a
rollback bundle was created for a source that proves not to be v13, that new
bundle is removed; a genuine v13 migration failure retains the private copies
for recovery.

## Cluster authorization and mirroring

Cluster authorization is opt-in. With gating disabled, accepted writes remain
local and the shipping seam is dormant. With gating enabled, a head-advancing
working operation is staged, its prospective digest is authorized through the
local criome, and it materializes only on a binding grant. Every terminal
non-grant discards the staged group and returns `AdvanceRefused`; reads remain
available.

After materialization, the mirror shipper drains versioned-log entries using
`spirit:sema:v14`. Restore verifies the expected head before importing a
checkpoint and suffix. Authorization controls acceptance; mirroring distributes
already accepted state.

## Release root

`flake.nix` is the single release root. It pins the daemon and judge contracts,
the judge implementation/configuration/provider, and exports
`mkUserServiceArtifacts`. A consumer pins Spirit once rather than assembling a
second release graph.

The release checks cover generated-artifact freshness, binary-only and NOTA
builds, runtime tests, migration readers and projection, guardian protocol,
configuration process boundaries, release-input alignment, and the service
bundle interface.

## Safety boundaries

- Tests and migration proofs use disposable stores only.
- The migration tool does not discover a live deployment or stop services.
- Deployment orchestration owns quiescence, exact target selection, activation,
  rollback choice, and retention policy for the private rollback directory.
- Current runtime code never opens the frozen v13 reader.
- Private diagnostics are redacted at the judge contract boundary.
