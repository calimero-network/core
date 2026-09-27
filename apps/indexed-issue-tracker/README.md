# indexed-issue-tracker

The canonical example for **`IndexedMap`**: an issue tracker whose list views
are index seeks instead of scans over every issue.

## The shape this collection is for

Nearly every list method an app writes has one shape: read the whole map,
filter by a field, sort, count. On an `UnorderedMap` each of those is `O(n)` in
every entry ever written, however few it returns. Here the value type declares
what it is looked up by:

```rust
#[derive(BorshSerialize, BorshDeserialize, AbiType, app::Mergeable, app::Indexed)]
#[borsh(crate = "calimero_sdk::borsh")]
#[index(status_created(status, created_at))]
pub struct Issue {
    #[index] pub status: LwwRegister<String>,
    #[index] pub assignee: LwwRegister<Option<String>>, // None: not in the index
    #[index] pub labels: LwwRegister<Vec<String>>,      // one row per label
    pub created_at: LwwRegister<u64>,
    pub title: LwwRegister<String>,
}
```

and each read costs what it returns:

| method | query | cost |
|---|---|---|
| `list_by_status` | `query("status_created").eq(status).desc().skip(o).limit(n)` | one reverse seek per row returned |
| `list_created_between` | `query("status_created").eq(status).range(from..to)` | a seek plus the rows in range |
| `list_by_label` | `query("labels").eq(label)` | the issues with that label |
| `list_by_assignee` | `query("assignee").eq(&Some(who))` | that person's issues |
| `status_counts` | `query("status").eq(s).count()` | index rows only; no issue is loaded |

Changing an indexed field goes through `update(id, |issue| ...)`, which keeps
every index in step with the change.

## What is synced, and what it costs

Only the issues. The indexes are node-local: the map stores exactly an
`UnorderedMap`'s bytes and reports itself as one, so nothing extra crosses the
wire or enters the root hash, and an app could switch an existing
`UnorderedMap` field to `IndexedMap` with no migration.

The price is paid after a peer's change. Sync applies it without telling the
indexes, so the first query afterwards notices, through a validity marker, that
the collection moved, and rebuilds: it reads every issue once and writes only
the index rows that changed. Every query after that is a seek again.

## The scenario

`workflows/indexed-issue-tracker.yml` runs two real nodes:

1. Node 2 never files anything. Its first query builds an index from issues it
   only received by sync.
2. Node 2 closes one issue and assigns another. Node 1's index was current a
   moment earlier; its next count and assignee lookup must reflect the peer's
   changes, not its own stale rows.
3. Node 1 files another issue and lists it straight away: a local write keeps a
   current index current.

```bash
PATH="$(../../scripts/setup-cargo-mero.sh):$PATH"
cargo mero build && cargo mero bundle --dev --no-icon
merobox bootstrap run workflows/indexed-issue-tracker.yml
```
