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
  -> schema-rust-next::RustEmitter
  -> checked-in generated module at src/schema/lib.rs
  -> engine/store/transport shims
```

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

1. reads a length-prefixed binary frame;
2. asks generated `Input` to triage by short header and decode itself;
3. decodes generated `Input`;
4. dispatches through `Engine`;
5. asks generated `Output` to frame itself as binary rkyv;
6. writes it back.

The hand-written transport module owns only length-prefix socket I/O. It does
not own route enums, short-header matching, or rkyv archive encode/decode.

### Nexus

`Engine::handle` is the Nexus entry point. Nexus is the runtime mail keeper:
when Signal input enters Nexus, `Input::message_sent` records the sent event,
`Input::dispatch_mail_with_nexus` wraps the payload as `NexusMail<Payload>`,
and the generated `InputNexus` trait dispatches to one method per Signal
variant. While Nexus owns that mail object, the message is being processed.

```text
Input -> NexusMail<Payload> -> SemaCommand -> SemaResponse -> MessageProcessed<Output> -> Output
```

The schema emits those nouns. Rust attaches the behavior:

- `NexusMail<Entry>::into_sema_command` and
  `NexusMail<Query>::into_sema_command` map Signal payload mail to state work.
- `SemaResponse::into_output` maps state response back to Signal reply.
- `Engine::handle` records generated `MailLedgerEvent` values for sent and
  processed mail and composes Nexus dispatch around the SEMA writer.
- `MessageSent::into_mail_ledger_event` and
  `MessageProcessed<Output>::processed_mail_event` attach runtime behavior to
  generated schema nouns instead of free helper functions.

### SEMA

`Store` is the current SEMA writer. The MVP store is still in memory, but all
state mutation goes through `Store::apply(SemaCommand)`. SEMA replies carry a
generated `DatabaseMarker` with `CommitSequence` and `StateDigest`, so Signal
outputs include the state marker that Nexus uses to close processed mail.
The next durable slice replaces the storage backend with redb without changing
the Nexus shape.

## Implementation methods

Schema-generated types are the implementation nouns. Hand-written runtime code
attaches behavior to those nouns or to state-owning runtime objects:

- `Input` is accepted by `Engine::handle`.
- `Input` emits `MessageSent` and dispatches as `NexusMail<Payload>`.
- `NexusMail<Payload>` lowers to generated `SemaCommand`.
- `SemaCommand` is applied by `Store::apply`.
- `SemaResponse` becomes generated `Output` carrying a `DatabaseMarker`.
- `MailLedgerEvent` stores sent and processed mail markers in the runtime
  ledger.
- `Input` and `Output` frame themselves at the Signal boundary.

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

`build.rs` lowers with `SchemaEngine::lower_source_with_context`, asserts that
the schema-next macro registry reached nested struct-field and enum-variant
macros, emits Rust into memory, and compares that output against
`src/schema/lib.rs`. The build fails if the checked-in generated source is
missing or stale. Runtime code imports `src/schema/lib.rs` directly; it does
not include generated Rust from `OUT_DIR`.

The schema-rust output path is already crate-relative (`src/schema/lib.rs`).
`build.rs` uses that path directly; it does not reinterpret a generated
`schema/lib.rs` path relative to `src/`.

## Known limits

- The schema language does not yet express vectors, so this pilot uses one
  topic per record.
- Storage and the mail ledger are in-memory, not redb.
- `StateDigest` is a deterministic prototype marker, not a content-addressed
  state hash.
- Schema diff/upgrade is absent.
- The repo-triad split (`spirit`, `signal-spirit`, `core-signal-spirit`) is
  not represented in this pilot repo.
- `MessageSent`, `NexusMail`, and `MessageProcessed` are generated by the Rust
  emitter's support surface rather than authored in a shared core schema.
- The next slice should make SEMA durable, make the mail support schema-authored,
  and start schema diff/upgrade.
