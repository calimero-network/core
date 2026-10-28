# authored-sorted-kv-store

A shared noticeboard on the component SDK (`calimero-app-sdk`): every note
belongs to whoever wrote it, and a reader takes a *slice* of the board (one
topic) instead of walking all of it. The v1 version was the example for
`AuthoredSortedMap`; here it is a `Table<Note>` and two explicit checks.

## The key shape

Keys are `"<topic>/<account>/<seq>"`:

- **topic**: what a reader asks for. `readTopic("news")` seeks the `byTopic`
  index, so notes under other topics cost nothing to skip.
- **account**: the author's, from `env::caller()`, never an argument, so a key
  names one note and its author and anyone can read it by key alone.
- **seq**: makes each of one author's writes its own key; two people posting
  `seq: "1"` to one topic land on two keys.

## The rules

- Any org member (or above) may `post`; guests get `Forbidden`.
- Only a note's author may `edit` or `retract` it: anyone else gets `NotFound`.
- Every write runs on the app's leader and is replicated as a log entry, so the
  check that ran there is the one every replica applies.

Insert is open and only an author may remove a note, so a member acting in bad
faith can grow the board without bound. `readTopic` reads only its topic's
slice, so a flood under another topic does not slow it; `flood` and `countAll`
demonstrate that.

## Methods

`me`, `post(topic, seq, text)`, `edit(key, text)`, `retract(key)`, `get(key)`,
`ownerOf(key)`, `readTopic(topic)` (posting order), `verifiedNotes(topic)`,
`myNotes(topic)` (the caller's keys, key order), `countAll`, `flood(topic, from, n)`.
`seq`, `from` and `n` are decimal strings.

## Running it

```bash
cargo test --manifest-path apps/authored-sorted-kv-store/Cargo.toml
```

`tests/harness.rs` holds the rules; `tests/scenarios.rs` runs
`scenarios/authored-sorted-kv-store.json` (the former merobox workflow: the
owner gate refused to the other account, both notes on one seq, the slice
unmoved by a 150-note flood) in the harness. The cluster runs the same file.
