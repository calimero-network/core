# indexed-forum

The harder `IndexedMap` example. Read
[`indexed-issue-tracker`](../indexed-issue-tracker) first: it covers
`IndexedMap` on its own. This one is a forum built from four fields, each
using a collection and write policy the others can't replace.

| state | collection | why this one |
|---|---|---|
| `charter` | `Frozen<String>` | written once in `init`; no node accepts a change or a removal, even from the founder |
| `posts` | `Moderated<IndexedMap<String, Post>>` | only a post's author may change it, and the author or a moderator may delete it, enforced by storage on every node; feeds filtered by board, tag, pin and author, without scanning |
| `comments` | `AuthoredSortedMap<String, LwwRegister<String>>` | only a comment's author may change it, enforced by storage on every node; a thread is one prefix slice |
| `votes` | `AuthoredMap<String, LwwRegister<bool>>` | one entry per voter per post, the voter's own: concurrent votes are never lost, and nobody votes or unvotes in someone else's name |

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

- **Posts and comments:** by storage, on every node. `Moderated<C>` and
  `Authored<C>` stamp each entry with the account that wrote it, over any keyed
  collection `C`: an `IndexedMap` for posts, a `SortedMap` for comments. A
  post's stamp also names the board's moderators, a writer set the founder
  starts in and rotates with `set_moderators`; `moderate_post` deletes anyone's
  post, and every node checks the deleter against the moderators as of that
  delete.
- **Charter:** by storage, on every node. `Frozen<String>` gives its creator a
  write-once capability and nothing else. A node applying a peer's
  write refuses an edit or removal by anyone else, including a node running
  patched code. The checks in `edit_post` and the rest just turn that refusal
  into a readable error before anything is written.
- **Votes:** by storage, on every node. A vote is the voter's own entry at
  the post's id, so nobody else can write it or remove it, and an account
  holds at most one per post. The score is `entries_at(post).len()`.

## A post id names its author

Storage keys an owned entry by its owner and its key, so two accounts can each
hold an entry at one key. If the client picked a bare id (`p1`), a read by id
would have to choose between them, and every rule for choosing ("the lowest
account", "the earliest `created_at`") is one a member can win on purpose, by
grinding an account or backdating a write. So:

- `create_post(id, ..)` mints and returns `"<caller's account hex>-<id>"`. The
  caller's `id` only ever collides with their own posts.
- Every read by id (`get_post`, `vote`, `comment`, `comments`) reads the entry
  of the account the id names, with `get_by`, and nothing else.
- A patched peer can still file an entry of its own at someone else's id. No
  read by id sees it, and the feeds drop any row whose owner stamp is not the
  account its id names, so a page holding such rows comes back short.
  `board_stats` counts index rows and can't tell, so it counts them.
- `delete_post` removes the caller's own post. `moderate_post` removes every
  account's entry at the id, with `remove_by`. Votes and comments stay, each
  its own author's; they are only read through a live post, so re-posting
  takes a new id.

A post also keeps its author as a field, because an index key has to come from
the value. The owner stamp is the truth: views show the stamp, and
`author_feed` drops any row whose field disagrees with it. That is what a
patched node writing a post under someone else's name would produce.

## Why the score isn't indexed

A score stored on the post would be a last-write-wins register. Two people
voting at once would each write `score + 1`, and one vote would be lost. One
entry per voter keeps every vote, but their count isn't a field of the post,
so it can't be an index key. There is no "top posts" query here for that
reason.

## What syncs

Only entries. The post indexes and the comment key order are node-local. A
peer's change lands without the indexes knowing, so the first post query after
it rebuilds them from the entries. Every query after that is a seek again.

## The scenario

`workflows/indexed-forum.yml` runs two nodes with two accounts:

1. Node 2 never files anything, then reads a `(board, tag)` feed. It builds the
   three-part index from synced posts, and a post with the tag in another board
   stays out.
2. Node 2 files and pins a post, votes on and comments on one of node 1's, by
   the id `create_post` returned. Node 1's counts and pinned strip reflect node
   2's writes, and node 1's own vote brings the count to two.
3. Node 2 can't pin node 1's post, and node 1 can't edit node 2's comment.
4. Node 1 files another post and finds it on top of the tag feed straight away.
5. Node 2 reads the charter. Node 2 can't moderate node 1's post, and node 1,
   the founder and moderator, removes node 2's spam: it leaves node 2's feed.

```bash
PATH="$(../../scripts/setup-cargo-mero.sh):$PATH"
cargo mero build && cargo mero bundle --dev --no-icon
merobox bootstrap run workflows/indexed-forum.yml
```
