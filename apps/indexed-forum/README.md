# indexed-forum

A forum on the component SDK: posts found by board, tag, pin and author through table
indexes, a post's comments as one range of an index, votes counted by a maintained count,
and a charter no method writes. Read [`indexed-issue-tracker`](../indexed-issue-tracker)
first for indexes on their own.

v1 built this from storage write policies that every node checked on applying a peer's
write. A component app runs on the app's leader, so each policy is a check in the method,
against `env::caller()`, before anything is written:

| state | v1 | here |
|---|---|---|
| charter | `Frozen<String>` | written in `init`; no method writes it |
| posts | `Moderated<IndexedMap<String, Post>>` | `Table<Post>`: only `post.author` changes or deletes; a row of `moderators` (the app owner, from `init`) removes anyone's (`moderatePost`) |
| tag feed | `#[index(board_tag_feed(board, tags, created_at))]` over a list | `Table<PostTag>`, one row per tag, indexed `(board, tag, createdAt)` |
| comments | `AuthoredSortedMap` | `Table<Comment>` keyed `"<post>/<createdAt:020>/<author>/<id>"`, indexed by post; only its author edits or deletes (anyone else: `NoComment`) |
| votes | `AuthoredMap` (one entry per voter) | `Table<Vote>` keyed `(post, voter)` with a `score` count per post |

| method | reads |
|---|---|
| `boardFeed { board, offset, limit }` | `boardFeed (board, createdAt)`, newest first |
| `boardTagFeed { board, tag, offset, limit }` | `boardTagFeed (board, tag, createdAt)`; other boards' posts with the tag are never read |
| `boardSince { board, after }` | `boardFeed` from `after + 1` |
| `pinned { board }` | `boardPinned (board, pinnedAt)` from 0 (unpinned rows hold none, which sorts first) |
| `authorFeed { author, limit }` | `authorFeed (author, createdAt)` |
| `boardStats { board }` | the `boardPins (board, pinned)` count; no post is loaded |

`createPost` mints the post id `"<caller>-<id>"`, so two accounts' `p1` are two posts.
Pages read at most 1000 rows, offset included. `u64` values (`createdAt`, `at`, `after`,
scores, counts) travel as decimal strings.

Errors: `Exists`, `NoPost`, `NoComment`, `NotAuthor`, `NotModerator` (each with its id or
key), `HasSeparator { value }`.

## Tests

`cargo test` runs `tests/harness.rs` (v1's tests, method for method) and
`scenarios/indexed-forum.json`, the merobox workflow as a scenario with exact results, in
the org of `indexed-forum.org.json`. `scenarios/forged-writes.json` replays what v1's
storage refused a patched peer (editing, retagging, pinning, deleting or moderating
someone else's post, editing or deleting their comment, taking back their vote) as
calls, each refused by the method's author or moderator check, with the post unchanged.
