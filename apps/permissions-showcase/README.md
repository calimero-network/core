# permissions-showcase

A team space with one write rule per kind of record, on the component SDK. v1 held each
field to a storage write policy that every node checked on applying a peer's write; a
component app runs on the app's leader, so each is a table plus an explicit check against
`env::caller()` before anything is written.

| record | v1 policy | the rule here |
|---|---|---|
| founding record, quorum | `Frozen<T>` | written in `init`; no method writes them |
| messages | `WriteOnce<SortedMap>` | keyed `(channel, id, author)`: posting again where the caller wrote is `AlreadyWritten`; nothing edits or removes one |
| announcements | `ModeratedOnce<UnorderedMap>` | keyed `(id, author)`, never edited; only a moderator (the app owner from `init`, and whom a moderator appoints with `addModerator`) removes one, not its author (`NotModerator`) |
| pages and revisions | `Authored<UnorderedMap<_, Page>>` with nested revisions | keyed by `(author, id)`: `renamePage`, `addRevision` and `deletePage` act on the caller's own (`NoPage` otherwise); `page { author, id }` names the author it reads |
| evidence | `ContentAddressed<IndexedMap>` | keyed by the SHA-256 of its length-prefixed `(kind, body)`: filing equal content again stores nothing; `evidenceOfKind` is one range of `byKind` |

Two accounts using one message, announcement or page id hold two independent rows, and
listings show one row per author. `u64` values (`sentAt`, `quorum`) travel as decimal
strings.

## Tests

`cargo test` runs `tests/harness.rs` (v1's tests, method for method) and
`scenarios/permissions-showcase.json`, the merobox workflow as a scenario with exact
results, in the org of `permissions-showcase.org.json`.
