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

## Runtime shape

The daemon owns an in-memory `Store`. The CLI:

1. reads one NOTA argument;
2. parses it into generated `Input`;
3. frames it as short-header + rkyv archive bytes;
4. sends it over a Unix socket;
5. decodes generated `Output`;
6. prints NOTA.

The daemon:

1. reads a length-prefixed binary frame;
2. triages by short header;
3. decodes generated `Input`;
4. dispatches through `Engine`;
5. frames generated `Output` as binary rkyv;
6. writes it back.

## Implementation methods

Schema-generated types are the implementation nouns. Hand-written runtime code
attaches behavior to those nouns or to state-owning runtime objects:

- `Input` is matched by `Engine::handle`.
- `Entry` is persisted by `Store::record`.
- `Query` is interpreted by `Store::observe`.
- `Output` is framed by the transport boundary.

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
- The signal frame, executor lowering, SEMA command/response, and async
  exchange identifiers are still hand-written shims or absent.
- The next slice should replace those shims with schema-derived code.
