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
cargo test -p calimero-node --test component_cluster views_search_a_components_tables_as_they_change
# Engine numbers (size, latency, memory, the search-gas fit): tools/search-bench
cargo run --release -p search-bench -- --sizes 10000,100000
```

## Modules

| File | What's there |
| --- | --- |
| `src/index.rs` | `ContextIndex`: opened over any tantivy `Directory` (merod uses one in memory), schema from a `SearchIndexSchema` (the node makes one per table: its declared fields as `Text`), idempotent delete-then-add `apply`, `commit(seq, root)` recording what it covers, `search` (words / prefix / substring on trigrams / fuzzy, keyword and range filters, relevance or a `u64` field order, a snippet from the field that matched). Limits: `MAX_LIMIT` 100 hits, `MAX_CURSOR` 10,000, `MAX_QUERY_LEN` 256 bytes |
| `src/tokenize.rs` | Words (Unicode segmentation, NFKD accent folding, lowercase, CJK bigrams) and trigrams; `TOKENIZER_VERSION` |

## How the index stays equal to state (in the node)

- A view brings the app's indexes up to the very state it reads before it runs:
  from the record keys the entries since the last update touched
  (`AppMachine::touched_after`, kept for the last `TOUCHED_RING` entries), else
  a full scan of the app's records. A document's id is the SHA-256 of the
  record's stored key.
- Indexes live in memory: a restart rebuilds them from state on first use.
- A changed schema, tokenizer, schema option or `FORMAT_VERSION` makes the
  index stale; it is wiped and rebuilt. It never enters the log or a snapshot.

## Invariants and Gotchas

- **The app comes from the caller, never the request.** The search host is
  bound to the view's app; `SearchRequest` has no app or context field. Keep it that way.
- **An index is derived data**: nothing replicated or hashed may depend on it.

Part of [crates/](../AGENTS.md).
