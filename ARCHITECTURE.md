# ARCHITECTURE — spirit-next

## Purpose

`spirit-next` is the running proof that schema can create an interface used by
a real CLI and daemon pair.

## Layers

```text
schema/spirit.schema
  -> build.rs
  -> schema-next::SchemaEngine
  -> schema-rust-next::RustEmitter
  -> generated module
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

The daemon is shaped as the Signal / Executor / SEMA runtime triad.

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

### Executor

`Engine::handle` is the executor entry point. It performs the runtime decision
shape explicitly:

```text
Input -> SemaCommand -> SemaResponse -> Output
```

The schema emits those nouns. Rust attaches the behavior:

- `Input::lower_to_sema` maps the external Signal request to state work.
- `SemaResponse::into_output` maps state response back to Signal reply.
- `Engine::handle` composes the two around the SEMA writer.

### SEMA

`Store` is the current SEMA writer. The MVP store is still in memory, but all
state mutation goes through `Store::apply(SemaCommand)`. The next durable slice
replaces the storage backend with redb without changing the executor shape.

## Implementation methods

Schema-generated types are the implementation nouns. Hand-written runtime code
attaches behavior to those nouns or to state-owning runtime objects:

- `Input` is matched by `Engine::handle`.
- `Input` lowers to generated `SemaCommand`.
- `SemaCommand` is applied by `Store::apply`.
- `SemaResponse` becomes generated `Output`.
- `Input` and `Output` frame themselves at the Signal boundary.

When a data shape changes, edit `schema/spirit.schema` first, then regenerate
through `build.rs`, then update the methods that act on the regenerated types.
Do not hand-write parallel type mirrors.

## Local stack testing

`scripts/check-local-schema-stack` runs the central local override test for
this pilot. It rebuilds `spirit-next` with local checkouts of `nota-next`,
`schema-next`, and `schema-rust-next` by overriding Nix source inputs. This is
the intended loop while improving the NOTA parser, schema lowering, or Rust
emitter: edit a substrate repo, run the consumer check here, and prove the
generated Rust still compiles and crosses the CLI/daemon rkyv boundary.

## Known limits

- The schema language does not yet express vectors, so this pilot uses one
  topic per record.
- Storage is in-memory, not redb.
- Async exchange identifiers are absent.
- Schema diff/upgrade is absent.
- The repo-triad split (`spirit`, `signal-spirit`, `core-signal-spirit`) is
  not represented in this pilot repo.
- The next slice should make SEMA durable and start schema diff/upgrade.
