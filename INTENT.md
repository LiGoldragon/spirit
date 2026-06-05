# INTENT — spirit

`spirit` proves a running Spirit-like component can be built from schema-derived interfaces. It is intentionally separate from production `spirit`/`persona-spirit` so operators can iterate without disturbing the deployed substrate.

Load-bearing constraints:

*CLI input and output are NOTA when the `nota-text` feature is enabled.* Component/process communication is always binary rkyv. Generated schema datatypes always carry rkyv support; NOTA encode/decode is an opt-in text-client surface, not a daemon requirement. The daemon binary must not depend on `nota-next`; the CLI crate enables `nota-text`. Tests run `cargo tree --edges normal --no-default-features` and assert `nota-next` is absent from the binary, while the text surface must contain it.

*Rust data types are generated from crate-local `schema/{signal,nexus,sema}.schema` plane schemas.* Authored schema source is a typed artifact before Rust emission. The shared generation driver reads each plane schema into `SchemaSource`, round-trips canonical source text and rkyv archive bytes through `SchemaSourceArtifact`, lowers from that typed source value to semantic `Schema`, and compares only the generated Rust artifacts with checked-in files. The source language has an in/out codec instead of being a one-way parser, and `.asschema` is no longer a checked component artifact.

*Schema namespaces are strict NOTA key-value maps.* Braces are key-value pairs. A namespace entry is a pair like `Topic String`, `Entry { Topics * Kind * ... }`, or `Kind [...]`. Struct fields are key-value pairs; `Topics *` reuses the same type, while `kind (Optional Kind)` binds a field to a different reference. Root enum bodies are square-bracket lists of exported object names. Namespace enum bodies use bare names for unit variants and parenthesized `(Variant Payload)` entries for data-carrying variants, such as `(Record Record)` or `(Sent Sent)`. Namespace bindings such as `Record Entry`, `RecordAccepted SemaReceipt`, and `SignalArrived Input` define the payload aliases those signatures reference. Bare bindings lower to aliases and direct enum payloads, not wrapper structs.

*The three runtime centers are concrete objects.* `SignalActor` handles admission, `Nexus` is the mail keeper and translator owning the store and ledger, and `Store` is the durable SEMA plane over `sema-engine`. `Engine` composes them and owns no SEMA state. Generated plane namespaces expose `signal::Input`/`signal::Output`, `nexus::Work`/`nexus::Action`, and `sema::WriteInput`/`sema::WriteOutput`/`sema::ReadInput`/`sema::ReadOutput`.

*Signal admission is explicit.* `SignalActor::admit` mints the origin route, validates generated `Input`, and creates `SignalAccepted`. Invalid input returns `Output::Rejected(SignalRejection { validation_error, database_marker })` where `ValidationError` is generated from schema; the runtime does not use a hand-written rejection enum.

*State is the first production-surface compatibility operation.* Generated
`Input::State(Statement)` carries raw psyche text. Signal only admits and
validates the non-empty statement. Nexus exposes classification as the
schema-declared `ClassifyState` effect command and `StateClassified` effect
result; the hand-written policy only implements that declared Nexus interface.
The classified `Entry` uses the production fallback policy (`unclassified`,
`Clarification`, `Minimum`, `Zero`) and SEMA persists it through the existing
`Record` write root. The generated canonical NOTA shape is `(State [text])`.
The CLI text edge also accepts deployed production shorthand `(State ([text]))`
and normalizes it to the generated input before binary framing.

*Nexus is the recursive runner payload keeper and the internal feature catalog.*
Signal triage produces a generated `nexus::Nexus<nexus::Work>` envelope;
`triad-runtime::Runner` owns the continuation budget and repeated dispatch.
Hand-written `Nexus` implements one decision step, SEMA write/read hooks, and
the effect hook. Per intent record `gvaz`, computations, result filters,
conditional writes, and similar internal engine features must appear as Nexus
schema verbs/objects before Rust implements them, so the daemon's feature
surface stays visible in `schema/nexus.schema` instead of hidden in ad-hoc
implementation code.
Per intent record `k4d9`, that schema visibility does not imply broad
crate-root export: the generated plane module is the schema API surface, while
the daemon crate root is only an ergonomic barrel and should not flatten every
internal Nexus noun unless a real consumer-facing API needs it.

*The daemon listener shell is shared runtime, not Spirit boilerplate.* `Daemon`
constructs a data-bearing `SpiritDaemonRuntime` around `Engine`, then hands it
to `triad_runtime::SingleListenerDaemon`. The shared runtime creates parent
directories, removes stale socket files, binds the Unix listener, starts and
stops the runtime, and keeps the listener alive after per-request transport
errors. Spirit owns only configuration decoding, engine construction, and the
generated signal-frame bridge for one accepted stream.

*SEMA is durable.* `Store` maps generated SEMA roots onto `sema-engine` identified-record operations over a `.sema` file. Each `Record` calls `Engine::assert_identified`, each `Remove` calls `Engine::retract_identified`, and `Observe`/`Lookup`/`Count` read through `Engine::match_identified`. SEMA replies carry generated `DatabaseMarker` values so Signal replies report the state commit sequence and digest.

*The daemon's single argument is a path to a binary rkyv `Configuration` object.* Text-facing launchers may create that file, but the daemon startup path only decodes binary state.

*Trace is optional runtime instrumentation.* The `testing-trace` surface observes Signal/Nexus/SEMA calls through generated trait hooks without affecting production binary behavior. Trace events carry schema-generated typed `ObjectName`, not free strings. Spirit owns the typed `TraceEvent` over plane-local object names; `triad-runtime` owns the reusable log, frame mechanics, and client-side collection.

Load-bearing proof: the real process boundary is tested, not only in-memory function calls. Trace events cross the Signal admission, `SignalEngine`, `NexusEngine`, and `SemaEngine` boundary, proving actor/interface use instead of source-string presence.
