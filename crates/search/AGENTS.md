# calimero-search - Per-App Full-Text Search

The index engine behind an app's search: one tantivy index per searchable table,
derived from the app's state and never replicated. An app opts in by naming a
table's text fields (`#[table(search = [..])]`); the node side, which keeps the
indexes and feeds them, is `calimero-node`'s `component::search::ComponentSearch`.
The user-facing guide is the docs site's
[Full-text search](../../docs/src/content/docs/build/guides/search.mdx) page.

## Package Identity

- **Crate**: `calimero-search`
- **Entry**: `src/lib.rs`
- **Key deps**: `tantivy` (index, BM25, fuzzy/prefix queries, snippets), `calimero-primitives` (the `search` types the node builds: `SearchIndexSchema`, `SearchDoc`, `SearchRequest`, `SearchResponse`), `unicode-normalization` / `unicode-segmentation` (the tokenizer)

## Commands

```bash
cargo test -p calimero-search
# End to end (3-node cluster, the kv component):
cargo test -p calimero-node --test it component_cluster::views_search_a_components_tables_as_they_change
# Engine numbers (size, latency, memory, the search-gas fit): crates/search/bench
cargo run --release -p calimero-search-bench -- --sizes 10000,100000
```

## Modules

| File | What's there |
| --- | --- |
| `src/index.rs` | `ContextIndex`: opened over any tantivy `Directory` (merod uses one in memory), schema from a `SearchIndexSchema` (the node makes one per table: its declared fields as `Text`), idempotent delete-then-add `apply`, `commit(seq, root)` recording what it covers, `searcher` (the last commit, point in time) and `search` / `search_in` (against the last commit / a searcher; words / prefix / substring on trigrams / fuzzy, keyword and range filters, relevance or a `u64` field order, a snippet from the field that matched). Limits: `MAX_LIMIT` 100 hits, `MAX_CURSOR` 10,000, `MAX_QUERY_LEN` 256 bytes |
| `src/records.rs` | A component app's table records as search documents, shared by the node and the SDK's test harness (review finding C10: the harness searches what a node would): `record_table`, `record_prefix`, `table_schema`, `doc_id`, `document` (declared text fields, a list of strings joined, cut at `MAX_FIELD_TEXT_BYTES`), and `TableSearch`, one table's in-memory index over one state (`build`, `search`, `key`), which `calimero-sdk`'s harness builds per view |
| `src/tokenize.rs` | Words (Unicode segmentation, NFKD accent folding, lowercase, CJK bigrams) and trigrams; `TOKENIZER_VERSION` |

## How the index stays equal to state (in the node)

- A view brings the app's indexes up to the very state it reads before it runs:
  from the record keys the entries since the last update touched
  (`AppMachine::touched_after`, kept for the last `TOUCHED_RING` entries), else
  a full scan of the app's records. A document's id is the SHA-256 of the
  record's stored key.
- A view searches the state it reads, never a later one (review finding C4): the
  views of an app share its indexes, so each takes a point-in-time snapshot of them
  (`ContextIndex::searcher`, queried with `ContextIndex::search_in`, plus the
  document keys) once they are brought up to its state, and every search it makes
  reads that. The views prepared on earlier states are brought up first, in order,
  so the indexes pass through each live view's state; a view on an older state than
  the indexes hold compares them with its own. The index is never behind a view's
  state (each view brings it up before it searches), so nothing waits for it.
  Tests: `calimero-node`'s `component::search::tests` (`a_view_searches_the_state_it_reads_while_a_later_view_moves_the_indexes`,
  `a_view_that_searches_after_a_later_view_did_searches_its_own_state`), `tests/writer.rs`
  (`a_searcher_reads_the_commit_it_was_taken_at`).
- Every document carries its digest (`doc_digest`, the `_h` fast field), and
  `ContextIndex::digests` reads them all back, so an index kept on disk is brought
  up to a state by indexing again only the documents whose digest differs.
- Indexes live in memory, or on a node's encrypted data disk
  (`calimero-node`'s `ComponentSearch::new`); one on disk is compared with the
  state on first use, never trusted as current.
- A changed schema, tokenizer, schema option or `FORMAT_VERSION` makes the
  index stale; it is wiped and rebuilt. It never enters the log or a snapshot.

## Invariants and Gotchas

- **The app comes from the caller, never the request.** The search host is
  bound to the view's app; `SearchRequest` has no app or context field. Keep it that way.
- **An index is derived data**: nothing replicated or hashed may depend on it.
- **A writer is opened by a write and kept until it is released or closed.** It
  holds three threads (indexing, segment updater, merges) and up to `WRITER_MEMORY`
  of arena while it indexes; `open` leaves none. `release_writer` lets it finish the
  merges its commits started on a thread of its own (the next write, `close_writer`
  or drop waits for that, as it holds the index's lock); `close_writer` waits in
  place. Dropping a writer abandons its merges. The node keeps writers only for the
  apps it refreshed last (`calimero-node`'s `MAX_OPEN_WRITERS`). Test: `tests/writer.rs`.

Part of [crates/](../AGENTS.md).
