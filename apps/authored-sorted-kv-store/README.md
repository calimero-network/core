# authored-sorted-kv-store

The canonical example for **`AuthoredSortedMap`**: a shared keyspace where every
entry is owned by whoever wrote it, and a reader can take a *slice* of the
keyspace instead of walking all of it.

Read it beside its two neighbours:

| app | what it shows |
|---|---|
| [`sorted-kv-store`](../sorted-kv-store) | the same ordered reads, no ownership — anyone may overwrite anything |
| [`kv-store-with-shared-storage`](../kv-store-with-shared-storage) | ownership by a *writer set* for the whole collection, rather than per entry |
| **this one** | ownership **per entry**, plus the ordered reads |

## The key shape, which is the whole lesson

Keys are `"<topic>/<account>/<seq>"`. Each of the three parts is load-bearing:

- **topic** — what a reader asks for. `read_topic("news")` is an index seek, not
  a scan, so notes under other topics cost nothing to skip.
- **account** — in the key, so a row that lies about its author disagrees with
  its own owner stamp and any reader can spot it with a string compare, without
  a metadata lookup per row. `Note.key_matches_owner` is that comparison.
- **seq** — makes each write its own key. Two people posting `seq: 1` to the
  same topic land on different keys and neither is refused. On a shared
  keyspace an occupied key is a key its owner holds **forever**, because only
  its owner may remove it — so a key two writers might share is a lock one of
  them can take permanently.

## Why the ordering is a safety property here, not a convenience

`AuthoredMap`'s only iteration is `entries()`, which loads every entry in the
collection. On an authored collection that is a liveness floor rather than a
speed one, because the two halves of the ownership model compound:

- **anyone may insert** under any key — insert is open by design, that is what
  makes the keyspace shared;
- **only an entry's own owner may ever remove it**.

So a member acting in bad faith can grow the collection without bound and
nobody else can shrink it — not the app, not the other members, ever. With only
a full scan available, every honest reader then pays for that on every read,
permanently. The entries never have to be *believed* to do the damage: an app
that correctly ignores all of them still reads all of them. Correct answers,
unusable app.

`flood()` is in the contract for exactly this reason. It is a demonstration,
not a feature — it is what that member does, and nothing in the contract can
stop them.

The cost is measured in `crates/storage/tests/read_cost_profile.rs`, in counted
store reads, fetching an 8-entry slice:

| entries in the collection | `AuthoredMap::entries()` | `AuthoredSortedMap::prefix()` |
|---|---|---|
| 250 | 500 | 17 |
| 1,000 | 2,000 | 17 |
| 4,000 | 8,000 | 17 |

Linear against flat. A writer who targets *your* prefix can still crowd it — no
collection prevents that — but an untargeted flood stops mattering.

## Running it

```bash
cargo test -p authored-sorted-kv-store          # the rules, in-process
cargo mero build --manifest-path apps/authored-sorted-kv-store/Cargo.toml
(cd apps/authored-sorted-kv-store && cargo mero bundle --dev --no-icon)
merobox bootstrap run workflows/authored-sorted-kv-store.yml   # two real nodes
merobox stop --all
```

`cargo test` proves the rules against one store. The merobox scenario proves
the three things one store cannot:

1. **Authored state replicates at all.** Every entry is `StorageType::User`,
   signed per action and verified against its owner inside
   `Interface::apply_action` on the receiving node.
2. **The owner gate holds across nodes.** Node 2 is a full member and may write
   as much as it likes; it still cannot edit or retract node 1's note. The
   local check inside `edit` binds only the node running it, so refusing it
   from a *different* node is the assertion that means something.
3. **The slice does not pay for the flood.** Node 2 writes 150 notes under a
   topic nobody reads; node 1's topic read comes back unchanged.
