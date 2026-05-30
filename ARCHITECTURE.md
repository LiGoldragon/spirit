# ARCHITECTURE — spirit-next

## Purpose

`spirit-next` is the running proof that schema can create an interface used by
a real CLI and daemon pair.

## Layers

```text
schema/lib.schema
  -> build.rs
  -> schema-next::SchemaPackage
  -> schema-next::SchemaEngine
  -> schema-next::MacroRegistry
  -> schema-rust-next::RustEmitter with opt-in NOTA surface
  -> checked-in generated module at src/schema/lib.rs
  -> engine composer + nexus mail keeper + durable redb store + transport
```

The generated module has one binary floor and one optional text surface.
`rkyv::Archive` / `Serialize` / `Deserialize` are always emitted because every
component speaks binary frames. `nota_next::NotaDecode` / `NotaEncode`, root
`FromStr`, root `Display`, and `to_nota` helpers are emitted behind the
`nota-text` feature. That lets the CLI crate target parse and print NOTA while
the daemon target compiles without the NOTA decoder linked into its runtime
surface.

The current `schema/lib.schema` intentionally uses the compact derived-member
surface: `@Topics` derives the `topics` field, `@RecordIdentifier` derives
`record_identifier`, and explicit bindings such as `kind@(Optional Kind)` stay
only where the field name differs from the referenced type. Single-reference
declarations (`Topic@String`, `RecordSet@{ (Vec Entry) }`) are newtypes in
asschema and emitted Rust.

The three runtime centers are concrete objects: `SignalActor` (admission),
`Nexus` (mail keeper + translator, owns the store + ledger), and `Store` (the
durable `.sema` redb database). `Engine` composes them and owns no SEMA state
of its own.

## Borrowed prototype lessons

- From `design-nota-from-schema`: make the recursion floor explicit and keep
  generated source as source, not hidden macro behavior.
- From operator `schema-rust-next`: schema emits Rust code first; Rust macros
  are a later ergonomic surface.
- From designer Spirit POC: keep actor/runtime boundaries visible, but avoid
  the retracted `EffectTable`/`FanOutTargets` authored-schema path.
- From the nspawn pipeline prototypes: prove the real process boundary, not
  only in-memory function calls.

## Runtime triad

The daemon is shaped as the Signal / Nexus / SEMA runtime triad.

### Signal

The CLI:

1. reads one NOTA argument;
2. parses it into generated `Input`;
3. asks generated `Input` to frame itself as short-header + rkyv archive
   bytes;
4. sends it over a Unix socket;
5. decodes generated `Output`;
6. prints NOTA.

The daemon:

1. starts from a path to a binary rkyv `Configuration` object;
2. opens the configured socket and `.sema` database path;
3. reads a length-prefixed binary frame;
4. asks generated `Input` to triage by short header and decode itself;
5. dispatches through `Engine`;
6. asks generated `Output` to frame itself as binary rkyv;
7. writes it back.

The daemon does not parse NOTA at startup and does not need `nota-next` for
its binary-only build. A text launcher or test can write the binary
configuration file; production configuration should later become another
typed binary signal surface differentiated by the root message enumerator,
not a NOTA side channel.

The hand-written transport module owns only length-prefix socket I/O. It does
not own route enums, short-header matching, or rkyv archive encode/decode.

### Nexus

`SignalActor::accept` is the Signal validation/admission point. Once a decoded
Signal root is accepted, it becomes `SignalAccepted`: the original generated
`signal::Signal<signal::Input>` plus a generated `MessageSent` lifecycle
event. `SignalAccepted` hands that object to the Nexus mail keeper by firing
the sent hook at the Signal→Nexus handoff and then passing the payload as
`NexusMail<Payload>` into `Nexus::process`.

`Nexus` is a real runtime object (a hand-written behaviour noun over the
schema-emitted types — Pattern C). It OWNS the durable SEMA `Store` handle and
the `MailLedger`, and it holds the mail in a TYPE-LEVEL being-processed state
across the SEMA call:

```text
Input -> Mail<BeingProcessed> { sema_input }   <- Nexus HOLDS the mail
  -> store.apply(sema_input)                     <- SEMA runs WHILE held
  -> Mail<Processed> { output }                  <- only run_sema produces this
  -> MessageProcessed<Output> emitted -> Output
```

`Mail<Phase>` is the typestate: both phases carry the generated message
identifier and minted `OriginRoute`; `Mail<BeingProcessed>` carries the lowered
`sema::Sema<sema::Input>` the mail will run; `Mail<Processed>` carries the
`signal::Signal<signal::Output>` the SEMA reply produced. The two phases hold
DIFFERENT data, so they are structurally distinct values, not one struct
wearing a marker. The only constructor of `Mail<Processed>` is the private
`Mail::<BeingProcessed>::run_sema`, which consumes the in-flight mail by value
and threads it through the store — so "Nexus holds the mail ⇒ it is being
processed" is a compile-time fact, not a log entry (record 970). The origin
route is the return address: generated `MessageSent`, `NexusMail<Payload>`,
`Mail<BeingProcessed>`, `Mail<Processed>`, and `MessageProcessed<Output>` all
carry the same route through the trip.

The schema emits the wire nouns. Rust attaches the behavior:

- `NexusMail<Entry>::into_nexus_input`,
  `NexusMail<Query>::into_nexus_input`, and
  `NexusMail<RecordIdentifier>::into_nexus_input` map Signal payload mail into
  the Nexus language; `Mail<BeingProcessed>: FromMail<Payload>` lowers each
  accepted mail into the SEMA language up front (the inbound half of the Nexus
  translation).
- `nexus::Nexus<nexus::Input>::into_nexus_output` maps Signal-side Nexus input
  to `sema::Input`, and maps `sema::Output` back to Signal `Output`;
  `run_sema` uses it for the outbound half.
- `Nexus` implements the generated `NexusEngine` trait, so tests can invoke
  the Nexus translation plane as `NexusEngine::execute(nexus::Nexus<nexus::Input>)
  -> nexus::Nexus<nexus::Output>` directly.
- `Engine` is a thin composer of the three centers (Signal admission + the
  Nexus mail keeper). It does NOT call the store directly — the SEMA invocation
  lives inside `Nexus::process`. Generated sent/processed lifecycle events are
  pushed through `MailLedgerHook` (the ledger Nexus owns), not by direct helper
  calls.
- `MessageSent::into_mail_ledger_event` and
  `MessageProcessed<Output>::processed_mail_event` attach runtime behavior to
  generated schema nouns instead of free helper functions, preserving the same
  origin route in sent and processed ledger events.

### SEMA

`Store` is the SEMA writer. SEMA means database work: the SEMA plane writes
durable state to the component database file (records 1007/1008). The store is
a real **redb** database written to a `*.sema` file:

- `Store::open(path)` creates or opens the `.sema` file and ensures the
  `records` and `ledger` tables.
- `SemaEngine::apply(sema::Sema<sema::Input>) -> sema::Sema<sema::Output>` is
  the only mutation surface. A
  `Record` is a redb write transaction that persists the rkyv-archived `Entry`
  in the `records` table (identifier -> archive) and advances the persisted
  `next-identifier` and `commit-sequence` counters in the `ledger` table. A
  `Remove` is a redb write transaction that deletes the record and advances
  the persisted `commit-sequence` when a record was present. An `Observe` is a
  redb read transaction scanning the `records` table and returning every
  matching entry.
- Entries carry `Topics`, a generated vector newtype. Queries carry
  `TopicMatch::{Partial,Full}` and an optional `Kind`: `Partial` accepts any
  requested topic, `Full` requires every requested topic, and `None` in the
  kind position searches only by topic.
- redb's transaction model gives crash-consistency: a store reopened from the
  same `.sema` path resumes its committed records AND its commit ledger, so the
  next write after a restart continues the sequence rather than restarting at 1.

SEMA replies carry a generated `DatabaseMarker` with `CommitSequence` and
`StateDigest`. `CommitSequence` is the persisted durable write counter.
`StateDigest` is a real content-addressed hash: blake3 over each committed
record's `(identifier, archived bytes)` folded with the commit sequence,
reduced to the schema's `Integer` width — an empty store digests to zero.
Signal outputs include the state marker that Nexus uses to close processed
mail.

The redb file lifecycle is owned by `Store` directly (synchronous redb API);
the daemon opens one `Store` for the process and shares it behind the `Nexus`
mutex. The kameo / `sema-engine` substrate is the destination for a
production component; this pilot uses redb directly to keep the proof
self-contained.

### Reuse

The schema declares reusable import/export nouns for language planes:
`Import {| Import sourcePath SourcePath localPath LocalPath |}` and
`Export {| Export localPath LocalPath publicPath PublicPath |}`.
The paths are single-colon namespaces, mirroring Rust crate/module paths with
`:` instead of `::`, for example `signal:sema:Magnitude`.

The same root shape applies to the three Spirit language planes in this pilot:
Signal (`Input`/`Output`), Nexus (`NexusInput`/`NexusOutput`), and SEMA
(`SemaInput`/`SemaOutput`). Each plane has imports/exports and a namespace
available to it; the implementation difference is which actor object owns the
method after the generated type exists.

The current `schema/lib.schema` spelling is the name-first `@` declaration
syntax: `Name@{...}` for struct-like declarations, `Name@[...]` for enum-like
declarations, and `name@Type` / `name@(Composite Type)` for member bindings.
Parentheses remain the composite/reference and macro-call argument shape.
That authored syntax lowers to the same `Asschema` roots and namespace before
`src/schema/lib.rs` is regenerated.

The generated Rust exposes plane namespaces over those bootstrap backing names:
`signal::Input`, `nexus::Input`, and `sema::Input` (plus matching `Output`).
Public execution signatures use the namespace-local names, for example
`sema::Sema<sema::Input>`, so the envelope carries the plane and payload names
stay short.

When code needs to branch across planes, it matches generated
`schema::Plane::{Signal,Nexus,Sema}`. Those variants carry the actual plane
envelopes, so the match surface and the message body stay one object.

## Implementation methods

Schema-generated types are the implementation nouns. Hand-written runtime code
attaches behavior to those nouns or to state-owning runtime objects:

- `Input` is accepted by `SignalActor::accept`, producing `SignalAccepted`.
- invalid `Input` is rejected as generated `Output::Rejected(SignalRejection)`
  before mail is sent or SEMA is touched.
- `SignalAccepted` emits `MessageSent` through a hook at the Signal→Nexus
  handoff and hands the payload to `Nexus::process` as `NexusMail<Payload>`.
- `Nexus` takes the mail into `Mail<BeingProcessed>`, holding it and its
  `OriginRoute` across SEMA.
- `NexusMail<Payload>` becomes generated `nexus::Nexus<nexus::Input>` (the
  lowering attached via `FromMail<Payload>` for `Mail<BeingProcessed>`).
- `nexus::Nexus<nexus::Input>` becomes generated `nexus::Nexus<nexus::Output>`.
- `nexus::Output::Sema` carries generated `sema::Input`.
- `sema::Sema<sema::Input>` is applied by `Store` through generated
  `SemaEngine`, writing the durable `.sema` redb database.
- `sema::Sema<sema::Output>` becomes `nexus::Input::Sema` and then generated
  `signal::Signal<signal::Output>` carrying a `DatabaseMarker`; this transition
  is `Mail::<BeingProcessed>::run_sema`, which produces the `Mail<Processed>`.
- `MailLedgerEvent` stores sent and processed mail markers, including their
  `OriginRoute`, in the ledger Nexus owns.
- `Input` and `Output` frame themselves at the Signal boundary.

This is the local version of the async mail keeper pattern. `SignalActor` is
the Signal admission object, `Nexus` is the data-bearing mail keeper that owns
the store + ledger and holds in-flight mail as a typestate, `MailLedger` is the
hookable lifecycle sink, and `Store` is the data-bearing durable SEMA writer.
`Engine` composes them. The generated mail nouns move between those objects;
the code must not replace that movement with module-level routing helpers.

When a data shape changes, edit `schema/lib.schema` first, then regenerate
through `build.rs`, then update the methods that act on the regenerated types.
Do not hand-write parallel type mirrors.

## Local stack testing

`scripts/check-local-schema-stack` runs the central local override test for
this pilot. It rebuilds `spirit-next` with local checkouts of `nota-next`,
`schema-next`, and `schema-rust-next` by overriding Nix source inputs. This is
the intended loop while improving the NOTA parser, schema lowering, or Rust
emitter: edit a substrate repo, run the consumer check here, and prove the
generated Rust still compiles and crosses the CLI/daemon rkyv boundary.

`build.rs` lowers with `SchemaEngine::lower_source`, emits Rust into memory,
and compares that output against `src/schema/lib.rs`. The build fails if the
checked-in generated source is missing or stale. Runtime code imports
`src/schema/lib.rs` directly; it does not include generated Rust from
`OUT_DIR`.

`build.rs` calls `RustEmitter::new(RustEmissionOptions::feature_gated_nota(
"nota-text"))`. The same schema-emitted data types can therefore be compiled
as binary-only daemon nouns or as dual NOTA+rkyv CLI nouns without hand-written
parallel mirrors. Cargo feature unification means a single Cargo invocation
cannot prove "CLI has NOTA, daemon lacks NOTA"; Nix builds the daemon and CLI
as separate package derivations and joins their binaries for integration
tests.

The schema-rust output path is already crate-relative (`src/schema/lib.rs`).
`build.rs` uses that path directly; it does not reinterpret a generated
`schema/lib.rs` path relative to `src/`.

Runtime-chain tests assert on schema-emitted objects, not test-local shadow
languages. Pattern A uses generated `MailLedgerEvent`, `NexusInput`,
`NexusOutput`, `SemaInput`, and `SemaOutput` as witnesses. The SEMA engine
test calls generated `SemaEngine::apply(sema::Sema<sema::Input>) ->
sema::Sema<sema::Output>`, and the full-chain test calls the durable
`SemaEngine::apply(sema::Sema<sema::Input>) -> sema::Sema<sema::Output>`
against a real `.sema` file, so each runtime plane remains typed as its schema
at both ends of the operation. The process-boundary tests parse the CLI's
stdout back through schema-emitted `Output::FromStr` (no raw-string digest
assertions, since the digest is now a real content hash), and one of them
proves durability at the real process boundary: a daemon writes the `.sema`
file, the process is killed, a fresh daemon opens the same `.sema` file, and
the recorded entry is still observable with the commit sequence resumed.
A dedicated store-reopen test in `runtime_triad.rs` proves the same durability
at the library level.

## Known limits

- The mail ledger is still in-memory (it is observability, not durable state):
  the `MailLedgerEvent` history resets on daemon restart. Only the SEMA records
  and commit ledger are durable.
- Schema diff/upgrade is absent (the generated `UpgradeFrom`/`AcceptPrevious`
  traits exist but nothing implements them yet).
- The repo-triad split (`spirit`, `signal-spirit`, `owner-signal-spirit`) is
  not represented in this pilot repo.
- `MessageSent`, `NexusMail`, and `MessageProcessed` are generated by the Rust
  emitter's support surface rather than authored in a shared core schema. The
  `Nexus` mail keeper, the `Mail<Phase>` typestate, and the redb `Store` are
  hand-written RUNTIME behaviour over the schema-emitted nouns (correct per
  records 999/1000); they are not boundary types and stay hand-written.
- `Store` shares one redb handle behind a mutex rather than a kameo
  single-writer actor; the `sema-engine` substrate is the production
  destination. The pilot uses redb directly to keep the proof self-contained.
- The next slice should make the mail support schema-authored, move the durable
  marker toward a shared `schema-core` type, and start schema diff/upgrade.
