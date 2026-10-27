# calimero-search - Per-App Full-Text Search

The index engine behind `env::search_query`: one tantivy index per `(app, index)`
pair, derived from the app's state and never replicated. Apps opt in through
the SDK (`#[derive(app::Searchable)]` + `app::search_indexes!`). The node side,
which keeps the indexes and feeds them, is `calimero-cloud-node`'s
`services::Search`. The user-facing guide is the docs site's
[Full-text search](../../docs/src/content/docs/build/guides/search.mdx) page.

## Package Identity

- **Crate**: `calimero-search`
- **Entry**: `src/lib.rs`
- **Key deps**: `tantivy` (index, BM25, fuzzy/prefix queries, snippets), `calimero-primitives` (`search` wire types shared with the SDK and the runtime), `unicode-normalization` / `unicode-segmentation` (the tokenizer)

## Commands

```bash
cargo test -p calimero-search
# End to end (3-node cluster, the real search-chat wasm):
cargo test -p calimero-cloud-node --test cluster views_search_the_apps_own_index_as_it_changes
```

## Modules

| File | What's there |
| --- | --- |
| `src/index.rs` | `ContextIndex`: opened over any tantivy `Directory` (merod uses one in memory), schema from the app's `SearchIndexSchema`, idempotent delete-then-add `apply`, `commit(seq, root)` recording what it covers, `search` (words / prefix / substring on trigrams / fuzzy, keyword and range filters, relevance or a `u64` field order, a snippet from the field that matched). Limits: `MAX_LIMIT` 100 hits, `MAX_CURSOR` 10,000, `MAX_QUERY_LEN` 256 bytes |
| `src/tokenize.rs` | Words (Unicode segmentation, NFKD accent folding, lowercase, CJK bigrams) and trigrams; `TOKENIZER_VERSION` |

## How the index stays equal to state (in the node)

- A view brings the app's indexes up to the very state it reads before it runs:
  from the entities the entries since the last update touched
  (`Outcome::touched`, kept for the last `TOUCHED_RING` entries), else a full
  scan of the app's state.
- Indexes live in memory: a restart rebuilds them from state on first use.
- A changed schema, tokenizer, schema option or `FORMAT_VERSION` makes the
  index stale; it is wiped and rebuilt. It never enters the log or a snapshot.

## Invariants and Gotchas

- **The app comes from the caller, never the request.** The search host is
  bound to the view's app; `SearchRequest` has no app or context field. Keep it that way.
- **An index is derived data**: nothing replicated or hashed may depend on it.

Part of [crates/](../AGENTS.md).
