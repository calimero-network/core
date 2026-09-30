# calimero-search - Per-Context Full-Text Search

Node-local, never-synced full-text search: one tantivy index per `(context, index)` pair, kept in the node's own store and brought up to date from state by the node's indexer. Apps opt in through the SDK (`#[derive(app::Searchable)]` + `app::search_indexes!`); views query through the `search_query` host function. The user-facing guide is the docs site's [Full-text search](../../docs/src/content/docs/build/guides/search.mdx) page.

## Package Identity

- **Crate**: `calimero-search`
- **Entry**: `src/lib.rs`
- **Key deps**: `tantivy` (index, BM25, fuzzy/prefix queries, snippets; `default-features = false` + `lz4-compression`), `calimero-store` (the `SearchIndex` / `SearchDirty` columns), `calimero-primitives` (`search` wire types shared with the SDK and the runtime), `unicode-normalization` / `unicode-segmentation` (the tokenizer)

## Commands

```bash
cargo test -p calimero-search
# The end-to-end suite (live ContextManager, the real search-chat wasm) lives in calimero-context:
cargo test -p calimero-context --lib search_tests
# Engine benchmarks (no wasm): tools/search-bench
cargo run --release -p search-bench -- --sizes 2000,10000
```

## Modules

| File | What's there |
| --- | --- |
| `src/directory.rs` | `RocksDirectory`: tantivy's `Directory` over `Column::SearchIndex`, files split in 64 KiB chunks keyed `context ‖ len(index) ‖ index ‖ len(file) ‖ file ‖ chunk_no`. `ChunkCache` (shared LRU of immutable chunks, lock table, open-handle counts so a merged-away file stays readable while a searcher holds it, per-context tally of deleted bytes for compaction). `delete_context`, `index_names` |
| `src/dirty.rs` | The dirty log (`Column::SearchDirty`): `stage` a row `{before, after, ids}` into the execution's transaction, numbered by a persisted per-context counter (`head`); `read_after`, `trim_through`, `contexts_with_rows` |
| `src/index.rs` | `ContextIndex`: schema from the app's `SearchIndexSchema`, idempotent delete-then-add `apply`, `commit(seq, root)` recording the dirty seq and state root covered in the commit payload, `search` (words / prefix / substring on trigrams / fuzzy, keyword and range filters, snippets). Limits: `MAX_LIMIT` 100 hits, `MAX_CURSOR` 10,000, `MAX_QUERY_LEN` 256 bytes |
| `src/service.rs` | `SearchService`: open indexes, `index_context` (the chain check and full builds), `search` (the query entry, context from the caller), `observe`/`reflects`, eviction (`evict_idle`), `compact`, `run_indexer`. `ContextSource`: what the node supplies (the app's exports, state root, context lock). `SearchConfig` |
| `src/tokenize.rs` | Words (Unicode segmentation, NFKD accent folding, lowercase, CJK bigrams) and trigrams; `TOKENIZER_VERSION` |

## How the index stays equal to state

1. **Same-batch dirty rows.** A committed execution of a search-enabled app (a local write or a peer's delta through `__calimero_sync_next`) stages one row in its own write batch: the state root before, the root after, the entity ids it touched. A crash loses both or neither.
2. **Root chain.** Every index commit records the root it reflects. The indexer replays rows only while each row's `before` equals the root reached so far, and, with no row left, checks the context's current root (read together with the log head under the context lock). Any mismatch means state moved without a row — a snapshot install, a HashComparison / level-wise repair, a migration, a run while search was off — and the index is rebuilt from a scan of state. Nothing on those paths needs to know search exists.
3. **Triggers.** Rows notify the indexer; a search view `observe`s the root it saw (so a context whose state arrived by snapshot is built on first use); every `audit_interval` the indexer checks each context it indexed since start. At start, contexts with a backlog are queued.
4. **Deletion.** `delete_context` (and the context manager's `purge_context_rows`) range-delete both columns; an app that stops declaring an index loses it on the next pass.

The index is derived data: a changed schema, tokenizer, schema option or `FORMAT_VERSION` opens the index as `Stale`, and it is wiped and rebuilt. It never enters a delta, a snapshot or the root hash.

## Invariants and Gotchas

- **The context comes from the caller, never the request.** `SearchService::search` takes the context from the runtime's `VMContext::context_id`; `SearchRequest` has no context field. Keep it that way.
- **Dirty seq is a counter, not a clock.** The bare `context(32)` key holds the highest seq staged; rows are `context ‖ seq BE` (40 bytes). The counter key is a prefix of every row key, so row scans never see it; `trim_through` never deletes it.
- **Stage under the exclusive lock.** `dirty::stage` reads the counter from the committed store; the execute path holds the context's write lock, so no other row can be staged in between.
- **Closing a writer drops what it has not committed.** `maintain` (writer close, eviction, compaction) is awaited between passes, never beside one.
- **An index some query holds is never evicted** (`Arc::strong_count == 1` gate).
- **`MAX_BUILDS_PER_PASS` = 2.** A context whose state keeps moving without rows under a build is retried next tick instead of starving every other one.

Part of [crates/](../AGENTS.md).
