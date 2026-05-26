# spirit-next

Runnable schema-derived Spirit pilot.

`spirit-next` proves the first practical version of the new architecture:

```text
schema/spirit.schema
  -> schema-next Asschema
  -> schema-rust-next generated Rust
  -> CLI NOTA input
  -> rkyv socket bytes
  -> daemon engine
  -> rkyv socket bytes
  -> CLI NOTA output
```

This is not production Spirit. It is the public pilot repo for making the
schema-created interface real at a process boundary.

## Run

Start a daemon with a single NOTA argument containing the socket path:

```sh
spirit-next-daemon "[/tmp/spirit-next.sock]"
```

Call it from the CLI:

```sh
SPIRIT_NEXT_SOCKET=/tmp/spirit-next.sock \
  spirit-next "(Record ([schema] Constraint [schema creates the interface] Maximum))"
```

The CLI accepts NOTA. The daemon socket carries length-prefixed rkyv bytes
with an 8-byte short header.
