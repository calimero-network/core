# forum

A forum on the component SDK: posts found by board, tag, pin and author through table
indexes, a post's comments as one range of an index, votes counted by a maintained count,
a board's chat whose messages are final, announcements only moderators take down, and a
charter no method writes. Read [`issue-tracker`](../issue-tracker) first
for indexes on their own.

v1 built this from storage write policies that every node checked on applying a peer's
write. A component app runs on the app's leader, so each policy is a check in the method,
against `env::caller()`, before anything is written:

| state | v1 | here |
|---|---|---|
| charter | `Frozen<String>` | the forum's `name` and `rules`, written in `init`; no method writes them |
| moderators | | `Table<Moderator>`: the app owner, from `init`; a moderator appoints another (`addModerator`) |
| posts | `Moderated<IndexedMap<String, Post>>` | `Table<Post>`: only `post.author` changes or deletes; a moderator removes anyone's (`moderatePost`) |
| tag feed | `#[index(board_tag_feed(board, tags, created_at))]` over a list | `Table<PostTag>`, one row per tag, indexed `(board, tag, createdAt)` |
| comments | `AuthoredSortedMap` | `Table<Comment>` keyed `"<post>/<createdAt:020>/<author>/<id>"`, indexed by post; only its author edits or deletes (anyone else: `NoComment`) |
| votes | `AuthoredMap` (one entry per voter) | `Table<Vote>` keyed `(post, voter)` with a `score` count per post |
| chat | `WriteOnce<SortedMap>` | `Table<Message>` keyed `(board, id, author)`, indexed `(board, sentAt)`: posting again where the caller wrote is `Exists`; nothing edits or removes a message, not even a moderator |
| announcements | `ModeratedOnce<UnorderedMap>` | `Table<Announcement>` keyed `(id, author)`: announcing again is `Exists`; only a moderator removes one (every author's at that id), not its author |

Every write needs an org member (or above); a guest reads and gets `Forbidden` for the
rest. The author of anything written is the caller, never an argument. A board is one
range of `boardFeed`, so a member filling another board (which only its author or a
moderator can empty) does not slow a reader of this one.

| method | reads |
|---|---|
| `boardFeed { board, offset, limit }` | `boardFeed (board, createdAt)`, newest first |
| `boardTagFeed { board, tag, offset, limit }` | `boardTagFeed (board, tag, createdAt)`; other boards' posts with the tag are never read |
| `boardSince { board, after }` | `boardFeed` from `after + 1` |
| `pinned { board }` | `boardPinned (board, pinnedAt)` from 0 (unpinned rows hold none, which sorts first) |
| `authorFeed { author, limit }` | `authorFeed (author, createdAt)` |
| `boardStats { board }` | the `boardPins (board, pinned)` count; no post is loaded |
| `messages { board }` | `chat (board, sentAt)`, oldest first; one row per author of an id |
| `announcements` | every announcement by id, one row per author |
| `charter`, `moderators` | `{ name, rules }`; the moderators' accounts |

`createPost` mints the post id `"<caller>-<id>"`, so two accounts' `p1` are two posts.
A post's tags come back sorted and de-duplicated (`createPost` and `retag` store them that
way; v1 kept them as given). `deletePost` and `moderatePost` remove the post's votes and
comments with it, so a post later filed at the same id starts with none
(`scenarios/repost.json`). `postMessage` returns the message's key, `"<board>/<id>"`.
Pages and listings read at most 1000 rows, offset included. `u64` values (`createdAt`,
`at`, `after`, `sentAt`, scores, counts) travel as decimal strings.

Errors: `Exists`, `NoPost`, `NoComment`, `NotAuthor`, `NotModerator` (each with its id or
key; `NotModerator` from `addModerator` names the account), `HasSeparator { value }`, and
the SDK's `Forbidden` for a guest's write.
Events: `Posted { id, board }`, `PostChanged { id }`, `PostDeleted { id }`,
`Commented { post, key }`, `Voted { post }`, `MessagePosted { key }`, `Announced { id }`,
`AnnouncementRemoved { id }`.

## Tests

`cargo test` runs `tests/harness.rs` (v1's tests, method for method, with those of the
noticeboard and team space this app absorbed: guests refused, a board read unmoved by a
300-post flood in another board, chat messages final, announcements removed only by a
moderator) and every scenario in the org of its `.org.json`.
`scenarios/forum.json` is the former merobox workflow as a scenario with exact results,
plus the chat, announcements and an appointed moderator. `scenarios/forged-writes.json`
replays what v1's storage refused a patched peer (editing, retagging, pinning, deleting or
moderating someone else's post, editing or deleting their comment, taking back their vote,
rewriting a chat message, removing an announcement or appointing oneself without being a
moderator) as calls, each refused by the method's check, with the data unchanged; and a
guest's writes refused with `Forbidden`. `scenarios/repost.json` files a post again at a
deleted or moderated id.
