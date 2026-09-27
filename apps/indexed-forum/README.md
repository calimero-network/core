# indexed-forum

The harder `IndexedMap` example. Read
[`indexed-issue-tracker`](../indexed-issue-tracker) first: it covers
`IndexedMap` on its own. This one is a forum built from three collections, each
used for what the other two can't do.

| state | collection | why this one |
|---|---|---|
| `posts` | `IndexedMap<String, Post>` | feeds filtered by board, tag, pin and author, without scanning |
| `comments` | `AuthoredSortedMap<String, LwwRegister<String>>` | only a comment's author may change it, enforced by storage on every node; a thread is one prefix slice |
| `votes` | `UnorderedMap<String, UnorderedSet<String>>` | a set of voter accounts merges by union, so concurrent votes are never lost |

## Posts: the index shapes

```rust
#[derive(BorshSerialize, BorshDeserialize, AbiType, app::Mergeable, app::Indexed)]
#[borsh(crate = "calimero_sdk::borsh")]
#[index(board_feed(board, created_at))]
#[index(board_tag_feed(board, tags, created_at))]
#[index(board_pinned(board, pinned_at))]
#[index(author_feed(author, created_at))]
pub struct Post {
    pub board: LwwRegister<String>,
    pub author: LwwRegister<String>,
    pub tags: LwwRegister<Vec<String>>,      // one row per tag
    pub pinned_at: LwwRegister<Option<u64>>, // None: no row
    pub created_at: LwwRegister<u64>,
    pub title: LwwRegister<String>,
    pub body: LwwRegister<String>,
}
```

| method | query | what it reads |
|---|---|---|
| `board_feed` | `query("board_feed").eq(board).desc().skip(o).limit(n)` | the page |
| `board_tag_feed` | `query("board_tag_feed").eq(board).eq(tag).desc()...` | the page; other boards' posts with that tag are never read |
| `board_since` | `query("board_feed").eq(board).range((Excluded(t), Unbounded))` | the new posts |
| `pinned` | `query("board_pinned").eq(board).desc()` | the pinned posts only |
| `author_feed` | `query("author_feed").eq(author).desc().limit(n)` | the page |
| `board_stats` | `.count()` on `board_feed` and `board_pinned` | index rows; no post is loaded |

`tags` holds several values in the middle of a compound key. The index stores
one row per tag, each `board ‖ tag ‖ created_at ‖ entry`. Pinning a board and a
tag still leaves a single ordered range.

## Where each gate is enforced

- **Comments:** by storage. An `AuthoredSortedMap` entry is owned by the account
  that wrote it, and every node refuses anyone else's update or removal. That
  includes a node running patched code.
- **Posts:** by this app's code only. `IndexedMap` entries carry no owner stamp,
  so `edit_post`, `retag`, `pin` and `delete_post` compare the caller with the
  stored author. Core has no collection yet that combines authored entries with
  secondary indexes.
- **Votes:** no gate. Each voter adds and removes only their own account.

## Why the score isn't indexed

A score stored on the post would be a last-write-wins register. Two people
voting at once would each write `score + 1`, and one vote would be lost. The
set of voters keeps every vote, but its size isn't a field of the post, so it
can't be an index key. There is no "top posts" query here for that reason.

## What syncs

Only entries. The post indexes and the comment key order are node-local. A
peer's change lands without the indexes knowing, so the first post query after
it rebuilds them from the entries. Every query after that is a seek again.

## The scenario

`workflows/indexed-forum.yml` runs two nodes with two accounts:

1. Node 2 never files anything, then reads a `(board, tag)` feed. It builds the
   three-part index from synced posts, and a post with the tag in another board
   stays out.
2. Node 2 files and pins a post, votes on and comments on one of node 1's. Node
   1's counts and pinned strip reflect node 2's writes, and node 1's own vote
   brings the count to two.
3. Node 2 can't pin node 1's post; the app refuses it. Node 1 can't edit node
   2's comment; storage refuses it.
4. Node 1 files another post and finds it on top of the tag feed straight away.

```bash
PATH="$(../../scripts/setup-cargo-mero.sh):$PATH"
cargo mero build && cargo mero bundle --dev --no-icon
merobox bootstrap run workflows/indexed-forum.yml
```
