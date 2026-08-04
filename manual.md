# Spirit manual

This is the current Spirit 0.27.0 object guide. The ordinary schema at
`signal-spirit@b37fc963292c157452d06e150296c19005dae3f2` and the owner schema
at `meta-signal-spirit@009cb6c8ddf985244189a79d554aa5d5c24605c8` are the
authoritative command and type definitions.

## Invocation and replies

Invoke `spirit` with exactly one ordinary Input object and `meta-spirit` with
exactly one owner Input object. The environment variables `SPIRIT_SOCKET` and
`SPIRIT_META_SOCKET` choose their respective Unix sockets. A bare atom is a
complete object. Flags, path operands, zero operands, and multiple operands are
rejected; neither help flags nor temporary `.nota` files are part of this
language.

```sh
SPIRIT_SOCKET=/run/user/1000/spirit.sock spirit Version
SPIRIT_META_SOCKET=/run/user/1000/meta-spirit.sock meta-spirit ObserveHead
```

Successful requests print one typed NOTA reply. Rejection is likewise typed:
ordinary requests can return `GuardianRejected`, `Rejected`, `AdvanceRefused`,
or `Error`; owner configuration can return `Rejected`. Transport/decode errors
go to stderr and return nonzero. Do not place provider prompts, provider
responses, deployed record bodies, secrets, or session material in transcripts.

## Ordinary objects

All record-writing shapes use this four-field entry and request justification:

```text
Entry         = ([(Technology (Software (Data SchemaEvolution)))] Constraint [description] Medium)
Justification = ([([evidence] None)] [reasoning])
Query         = (Any Any Any None Any)
```

The ordinary Input roots are exactly the following 21 roots. These are
canonical object forms (replace illustrative identifiers and text with typed
values suitable for the request):

```text
State                 (State [statement])
Record                (Record (([domains] Constraint [description] Medium) ([([evidence] None)] [reasoning])))
Propose               (Propose (([domains] Principle [description] Medium) ([([evidence] None)] [reasoning])))
Clarify               (Clarify (record-id [clarifying description] ([([evidence] None)] [reasoning])))
Supersede             (Supersede ([record-id] [([domains] Decision [replacement] Medium)] ([([evidence] None)] [reasoning])))
Retire                (Retire (record-id ([([evidence] None)] [reasoning])))
ResolveClarification  (ResolveClarification (clarification-id [(record-id [replacement])] ([([evidence] None)] [reasoning])))
Observe               (Observe (Any Any Any None Any))
TextSearch            (TextSearch [description words])
Lookup                (Lookup record-id)
Count                 (Count (Any Any Any None Any))
BumpImportance        (BumpImportance record-id)
ChangeRecord          (ChangeRecord (record-id ([domains] Constraint [replacement] High) ([([evidence] None)] [reasoning])))
LookupStash           (LookupStash 1)
Tap                   (Tap All)
Untap                 (Untap 1)
ApplyAuthorizedRecord (ApplyAuthorizedRecord (record-id [versioned-entry-hex] [authorization-evidence-hex]))
SubscribeIntent       (SubscribeIntent (Any Any Any None Any))
Version               Version
Marker                Marker
Intent                (Intent [All])
```

`Observe` and `Count` use the five-predicate `Query` in this order:
`DomainMatch`, `KeywordMatch`, `TextMatch`, `SelectedKind`, and
`ImportanceSelection`. Domain matches are `Any`, `(Partial [scopes])`, or
`(Full [scopes])`; keyword matches are `Any`, `(AnyKeyword [keywords])`, or
`(AllKeywords [keywords])`; text matches are `Any` or `(ContainsText [text])`;
kind is `None` or `(Some Kind)`; importance is `Any`, `(ExactImportance
Magnitude)`, `(AtMostImportance Magnitude)`, or `(AtLeastImportance
Magnitude)`.

`TextSearch` is the compact ranked description lookup. `Lookup` resolves one
identifier. `LookupStash` reopens a result set returned by `Observe`.
`SubscribeIntent` emits `Event` replies after the initial subscription reply.
`Tap` and `Untap` manage operation observation. `ApplyAuthorizedRecord` is the
typed cluster-authorized write path; ordinary callers should not fabricate its
evidence.

## Owner objects

The owner-only meta socket has exactly four roots:

```text
Configure         (Configure (Default None None None))
Import            (Import [(record-id ([domains] Constraint [description] Medium))])
ObserveHead       ObserveHead
ObserveHeadObject ObserveHeadObject
```

`Configure` positions are archive target, optional mirror target, optional
Criome gate target, and optional guardian prompt target. `Import` contains
identifier-plus-four-field-entry values. `ObserveHead` reports marker and
optional digest; `ObserveHeadObject` reports marker and optional serialized
head object. These owner controls are not ordinary capture operations.

## Data, lifecycle, and operations safety

An active record is only `Entry { Domains Kind Description Importance }`.
`RecordRequest` and `Proposal` add a justification at the operation boundary.
There are no active certainty, core privacy, referent registration, relation,
public/private class, default/private shorthand, `PublicTextSearch`,
`ChangeCertainty`, `ChangePrivacy`, or `CollectRemovalCandidates` operations.

`ChangeRecord` retains an identifier while replacing its four-field entry.
`Retire`, `Supersede`, and `ResolveClarification` name their targets and place
retained data in the lifecycle archive before retraction. Records and archive
contents are operationally confidential even though core Spirit has no privacy
field: keep owner sockets, store locations, credentials, session references,
provider material, raw corpus bodies, and diagnostics closed or redacted.

The daemon speaks binary Signal frames and requires its immutable binary
configuration artifact. `spirit-write-configuration` and
`spirit-migrate-store` are maintenance/service interfaces, not public grammar
examples for `spirit` or `meta-spirit`.
