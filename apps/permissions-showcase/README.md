# permissions-showcase

A team space with one field per write policy the storage layer enforces. Each
rule is checked by every node when it applies a peer's write, so a node running
patched code gets the same refusals as the API gives an honest one.

| field | type | who may change it |
|---|---|---|
| `founding` | `Frozen<Founding>` | nobody: written once in `init`, the founder included |
| `quorum` | `Frozen<u64>` | nobody |
| `messages` | `WriteOnce<SortedMap<String, Message>>` | nobody: the author owns a message and cannot edit or delete it |
| `announcements` | `ModeratedOnce<UnorderedMap<String, Message>>` | nobody edits one; a moderator removes one, its author cannot |
| `pages` | `Authored<UnorderedMap<String, Page>>` | the author, including the `revisions` collection nested in the page |
| `evidence` | `ContentAddressed<IndexedMap<[u8; 32], Evidence>>` | nobody: keyed by the hash of its bytes |

The last policy, `Moderated<C>` (the author edits and deletes, a moderator
deletes), is in [`indexed-forum`](../indexed-forum).

The methods never check the caller themselves: every refusal the tests and
the scenario assert comes from the collection.

## What each read costs

- `channel(c)` is one prefix slice of the sorted map: `"<channel>/<id>"` keys.
- `evidence_of_kind(k)` is one index seek; `evidence(hash)` is one lookup.
- `announcements()` reads every announcement, which stays small because only
  moderators can remove them and everyone can see them.

## Nested collections

A page's `revisions` is a collection inside the page's entry. Its entries are
stamped with the page's owner, so another account can add, change or remove
none of them, on any node. Inside an immutable entry (`WriteOnce`,
`ModeratedOnce`, `ContentAddressed`) a nested collection can only be empty, and
nothing can ever be written into it.

## The scenario

`workflows/permissions-showcase.yml` runs two nodes with two accounts:

1. Node 2 reads the founding record and quorum node 1 froze.
2. Node 2 cannot post under node 1's message key; each node's messages show up
   in the other's channel.
3. Node 2 cannot remove its own announcement; node 1, the founder, removes it,
   and node 2 sees it gone.
4. Node 2 cannot appoint itself a moderator. Node 1 appoints it, and node 2
   then removes node 1's announcement.
5. Node 2 can neither rename node 1's page nor add a revision to it; node 1's
   revision reaches node 2.
6. Node 2 files evidence, node 1 finds it by kind, and filing the same content
   on node 1 gives the same hash.

```bash
PATH="$(../../scripts/setup-cargo-mero.sh):$PATH"
cargo mero build && cargo mero bundle --dev --no-icon
merobox bootstrap run workflows/permissions-showcase.yml
```
