# search-poc: per-context full-text search on the node

A proof of concept for full-text search in Calimero apps, with the numbers it
produced. The motivating apps are mero-chat (message search) and mero-docs (text
search). Today both scan every item inside WASM, and past a few thousand items
that scan runs out of the 1e9 gas budget. The PoC moves indexing out of WASM:
each node keeps one [tantivy](https://github.com/quickwit-oss/tantivy) index per
context in its own store, and a WASM view queries that index through a view-only
host function.

What is in the branch:

| Piece | Where |
|---|---|
| Store-backed tantivy index, dirty log, indexer, query entry | `crates/search` |
| Wire types (schema, extract/scan, request/response) | `crates/primitives/src/search.rs` |
| `search_query` host function (views only, bound to the running context) | `crates/runtime/src/logic/host_functions/search.rs` |
| Dirty row staged in the execute batch; `NodeExtractor` (app views); `SearchHostAdapter` | `crates/context/src/search.rs`, `crates/context/src/handlers/execute/mod.rs` |
| `env::search` for guests | `crates/sdk/src/env.rs` |
| Opt-in on the node (`CALIMERO_SEARCH_POC=1`) | `crates/node/src/run.rs` |
| Test app | `apps/search-chat` |
| End-to-end tests and benchmark (live `ContextManager`, real wasm, RocksDB) | `crates/context/src/handlers/execute/search_poc_tests.rs` |
| Engine benchmark harness | `tools/search-poc` (this directory); raw report in `results/engine.md` |

## Design

### Where the index lives

- **One index per `(context, index name)`, only on this node.** The index is
  derived data. It never enters a delta, a snapshot or the root hash, and every
  node builds its own from the state it holds.
- **It is stored in the node's own RocksDB**, in a new `SearchIndex` column, not in
  files. `RocksDirectory` (`crates/search/src/directory.rs`) implements tantivy's
  `Directory`. It splits each index file into 64 KiB chunks keyed
  `context ‖ index ‖ file ‖ chunk_no`. So the index:
  - is encrypted at rest along with the rest of the store;
  - shares one WAL with the state it is derived from;
  - is dropped for a deleted context with one range delete
    (`SearchService::delete_context`, called from `delete_context.rs`).
- Segment files never change once written, so their chunks sit in a
  `ChunkCache` shared by every directory (32 MiB by default). tantivy rewrites
  only `meta.json` and `.managed.json`, and each rewrite is one atomic write
  batch.

### Keeping the index consistent with state: the same-batch dirty log

1. When a run commits, whether a local write or a peer's delta, the execute path
   reads the entity ids it changed from the `StorageDelta` the run already
   produced (`changed_entity_ids`).
2. It stages one `SearchDirty` row (`context ‖ seq`, holding those ids) into the
   **same transaction** as the state change (`ContextStorage::stage_search_dirty`).
   A crash therefore loses both or neither, never just the row. This happens only
   for an app that exports `search_poc_extract`; other apps write nothing.
3. After the commit the context manager calls `SearchService::notify`. Every
   250 ms the indexer folds the pending rows of each notified context into one
   tantivy commit. It records the last seq it covered in the commit payload, and
   only then trims those rows.
4. The node cannot tell what an entity id *means*, so the indexer asks the app. It
   runs the app's `search_poc_extract` view read-only against current state,
   which returns one document or `None` per id. `None` becomes a delete. Every
   write is an idempotent delete-then-add, so replaying a row is harmless. After a
   crash, the indexer resumes from the seq in the last commit payload.
5. A full build takes one of three triggers: the first time the index opens, a
   schema or version bump from the app, or a tokenizer or option change. It
   records the current last seq, clears the index, pages through the app's
   `search_poc_scan` view, and replays whatever landed while it ran.

### Peer deltas

A peer's delta runs through the same `internal_execute` commit block as a local
write. The delta payload is decoded for its changed ids before the apply, and the
dirty row goes into the apply's own batch. No sync-path code changes. The test
`a_peer_delta_marks_its_entities_dirty_too` applies a real artifact from one
context to another and checks the row and the hit.

### The host function contract, and why it is view-only

`search_query(request_ptr, register_id) -> u32` takes a borsh `SearchRequest`: an
index name, a query, a mode (`words`, `prefix`, `substring` or `fuzzy`),
equality filters, a cursor and a limit. It returns a borsh `SearchResponse`
(entity id, score and snippet per hit, the total, the next cursor), or `0` with a
reason. The SDK wraps it as `env::search`.

- **There is no context argument.** The host passes its own
  `VMContext::context_id`. A guest can name only an index of the context it runs
  in.
- **Views only.** The node hands a run the `SearchHost` handle only when the run
  is read-only (`is_read_only_call`, i.e. an `#[app::view]` on the read-only
  runner). Anything else traps with `SearchUnavailable`. There are three
  reasons:
  - The index is node-local and lags state by up to one commit interval. A write
    that branched on it would record in a delta a decision other nodes cannot
    reproduce or check.
  - Gas does not meter host-side work. The PoC bounds it with at most 32 calls per
    execution, a request of at most 4 KiB, a query of at most 256 bytes, at most
    100 hits per page and a cursor of at most 10,000. A view already runs under
    the shared lock, so none of that sits on the write path.
  - A trap in a write would roll back the whole execution. In a view it rolls back
    only a read.
- **A hit is a pointer, not a value.** The view re-reads each hit's entity from
  state (`UnorderedMap::get_by_entity_id`) and drops any that are gone. What a
  client finally sees is always current state, filtered by the app's own code.

## Isolation guarantees and their tests

Every test named here passes on the branch (see "State of the branch" below).

| Guarantee | Enforced by | Tests |
|---|---|---|
| A context never reads another context's index through `search_query` | The host fills the context in from `VMContext::context_id`. The request type has no context field. Index keys are prefixed by context. | `a_view_only_ever_searches_its_own_context` (e2e, two contexts, same message key in both); `the_host_binds_every_call_to_the_running_context` (runtime unit); `a_context_never_sees_another_contexts_hits` and `scores_in_one_context_ignore_every_other_context` (`crates/search/tests/service.rs`); `two_contexts_never_see_each_others_files` (directory unit) |
| A search never returns a deleted item | The view re-reads every hit and counts the missing ones as `stale`. The indexer turns a `None` extract into a delete. A full build scans only live state. | `a_deleted_message_is_never_returned` (e2e: local delete and a peer-delta delete, before and after the indexer runs, and after a full rebuild); `messages_are_indexed_from_the_dirty_log_and_found_by_the_view` (edit and delete while the index lags); `edits_and_deletes_land_and_a_crash_replays_from_the_payload` (service); `get_by_entity_id_reads_only_this_maps_live_entries` (storage unit) |
| A search never returns an unauthorized item | A hit is an entity id that goes back through the app's own read path (`get_by_entity_id`), which returns `None` for anything that is not a live entry of *that* collection (another collection's entry, the collection itself, a forged id). Only views may search, so the index can never feed a write. Without search on the node, the view is refused. | `get_by_entity_id_reads_only_this_maps_live_entries`; `a_write_can_never_search` (e2e: a mutating method that searches traps, and nothing commits); `a_run_without_the_search_handle_cannot_search` (runtime unit); `without_search_nothing_is_written_and_the_view_is_refused` (e2e); `calls_are_capped_per_execution` (runtime unit) |
| Deleting a context drops its index and dirty log | `delete_context` range-deletes both | `deleting_a_context_drops_its_index_and_log` (service) |

What "unauthorized" covers here is limited by what the PoC app models. The
index is built by running the extract view as this node's own member identity.
So anything this node's state holds for the context can be indexed, and a hit is
filtered only by the app's read path when the hit is shown. An app with per-user
visibility inside one context would have to enforce it in the view that returns
hits. The PoC's app has no such rule, so it is untested (see the open questions).

## Benchmarks

**Machine:** Intel Xeon @ 2.80 GHz, 4 vCPUs, 15 GiB RAM, Linux 6.18 (a cloud
container). **Build:** the workspace `release` profile (`lto = "fat"`,
`opt-level = "z"`, `codegen-units = 1`), with the app built by
`cargo mero build` (`app-release` plus `wasm-opt -Oz`). Every number below is
from a run on this branch. Nothing is extrapolated.

Two layers were measured:

- **End to end** (`search_poc_bench`): a live `ContextManager` on RocksDB running
  the real `search-chat` wasm. It covers seeding through `post_many`, indexing
  through the app's extract and scan views, queries through the app's `search`
  view (wasm, then the host function, then a re-read of each hit), and the
  in-WASM scan baseline (`scan_search`, which lowercases every message and tests
  `contains`).
- **Engine** (`tools/search-poc`): `ContextIndex` and `SearchService` over the same
  `RocksDirectory` and a real RocksDB store, beside tantivy's stock
  `MmapDirectory`. There is no WASM. It gives index size, raw query latency,
  memory and crash replay, and it goes up to 200,000 messages.

Corpus: synthetic chat messages, Zipf-like over a 4,096-word vocabulary. An
end-to-end message is ten six-letter words, plus `needle` in 1 message in 200
and `zebrafish` in 1 in 2,000. The engine corpus averages 11 words and 66 to 67
bytes of text. `zebrafish` is the rare term, `kakaka` (in about 15% of messages)
the common term, and `qqxqq` (in none) the no-match term.

### Reproduce

```bash
PATH="$(scripts/setup-cargo-mero.sh):$PATH"
cargo mero build --manifest-path apps/search-chat/Cargo.toml
SEARCH_POC_N=10000 cargo test --release -p calimero-context --lib search_poc_bench -- --ignored --nocapture
cargo run --release -p search-poc -- --sizes 2000,10000,50000,200000
```

### Index build (end to end, through the wasm views)

| messages | full build from a scan of state | per message | incremental drain of the seeding backlog | per message |
|---|---|---|---|---|
| 2,000 | 0.25 s | 124 µs | 0.24 s | 119 µs |
| 10,000 | 0.96 s | 96 µs | 0.95 s | 95 µs |
| 50,000 | 4.81 s | 96 µs | 5.65 s | 113 µs |
| 200,000 | 25.62 s | 128 µs | 30.24 s | 151 µs |

Most of the per-message time goes to extraction through wasm (73 to 139 µs), not
tantivy (10 to 48 µs). Engine-only builds with no wasm take 76 to 101 µs per
message with trigrams and 52 to 61 µs without (table below).

**A bug the benchmark found and the branch fixes.** Before commit `b2113f715`,
the scan view listed every entity id on every page, which is O(n) per page. At
50,000 messages a single page exhausted the gas budget, so a full build could
never finish. At 10,000 messages it took 237 µs per message. Pages now walk only
the occupied child-trie buckets from a cursor (`ChildTrie::children_from`,
`UnorderedMap::entity_ids_from`), so a page costs the size of its own contents.

### Write path: cost added to `send_message` (`post`)

| messages | `post` p50 / p95, search off | `post` p50 / p95, search on | gas off / on | dirty row | indexing that post later (off the write path) |
|---|---|---|---|---|---|
| 2,000 | 5.62 / 8.16 ms | 5.80 / 10.25 ms | 2.50 M / 2.50 M | 140 B | 252 µs |
| 10,000 | 5.51 / 9.78 ms | 5.71 / 8.44 ms | 2.53 M / 2.53 M | 140 B | 211 µs |
| 50,000 | 7.24 / 11.64 ms | 6.94 / 9.92 ms | 2.59 M / 2.59 M | 140 B | 296 µs |
| 200,000 | 8.47 / 13.90 ms | 9.03 / 12.64 ms | 2.67 M / 2.67 M | 140 B | 408 µs |

- **Gas is identical with search on.** The dirty row is written by the host
  after the run, so the guest does no extra work.
- **Wall time is within run-to-run noise.** Across 200 posts, on and off are not
  consistently ordered at any size.
- The extra bytes per execution are one 140-byte row (3 entity ids).
- Indexing the post costs about 0.2 to 0.4 ms per post, paid later by the indexer
  and batched into a single commit. Of that, about 70 to 120 µs is the extract
  view and about 130 to 320 µs is tantivy (the upper end at 200,000 documents).
- The engine benchmark measured a single message committed on its own at 11 to 22 ms.
  That cost is why the indexer batches its commits every 250 ms.
- **Freshness** (post returned, then searchable, through the live indexer loop):
  p50 about 150 to 160 ms, p95 about 255 to 265 ms, max 275 ms at every size
  from 2,000 to 200,000.

`search-chat` is not mero-chat. Its `post` is a flat `UnorderedMap` insert of
about 2.5 M gas at any size. The 1.55 G gas `send_message` in mero-chat at 5,000
messages comes from mero-chat's `AuthoredVector` walk, which search neither
causes nor fixes. That needs mero-chat's own storage fix (see
`crates/runtime/tests/chat_wall.rs`).

### Queries through the app's `search` view (end to end, top 20 re-read from state)

p50 / p95, and the gas of the whole view:

| query | 2,000 msgs | 10,000 msgs | 50,000 msgs | 200,000 msgs |
|---|---|---|---|---|
| floor: `count` view, no search | 2.21 / 3.43 ms (0.09 M) | 1.77 / 2.29 ms (0.09 M) | 2.60 / 5.54 ms (0.09 M) | 3.41 / 5.95 ms (0.09 M) |
| rare word `zebrafish` | 2.77 / 7.08 ms (0.20 M), 2 hits | 2.44 / 2.97 ms (0.46 M), 6 hits | 4.73 / 12.69 ms (1.37 M), 26 hits | 6.03 / 11.16 ms (1.37 M), 101 hits |
| common word `kakaka` | 3.99 / 8.08 ms (1.32 M), 314 hits | 3.57 / 6.95 ms (1.36 M), 1,475 hits | 4.90 / 10.85 ms (1.37 M), 7,414 hits | 7.18 / 15.18 ms (1.37 M), 29,373 hits |
| no match `qqxqq` | 2.29 / 4.84 ms (0.07 M) | 1.81 / 2.16 ms (0.07 M) | 2.41 / 5.78 ms (0.07 M) | 3.24 / 7.91 ms (0.07 M) |
| prefix `needl` | 3.26 / 5.02 ms (0.71 M) | 3.09 / 5.09 ms (1.25 M) | 4.80 / 10.25 ms (1.25 M) | 6.04 / 10.37 ms (1.25 M) |
| substring `eedl` | 2.86 / 4.59 ms (0.71 M) | 3.54 / 6.64 ms (1.24 M) | 4.29 / 7.56 ms (1.24 M) | 6.12 / 14.41 ms (1.24 M) |
| fuzzy `neadle` | 2.83 / 3.47 ms (0.71 M) | 3.23 / 5.87 ms (1.25 M) | 4.62 / 6.38 ms (1.25 M) | 5.37 / 6.22 ms (1.25 M) |

The view's gas is bounded by the page, never by the corpus. It tops out at about
1.37 M, spent on re-reading 20 hits. Host-side work is not metered (see the
limits below).

### Baseline: the in-WASM scan (`scan_search`, same app, same state)

| messages | rare p50 / p95 | common p50 / p95 | no match p50 / p95 | gas |
|---|---|---|---|---|
| 2,000 | 105 / 115 ms | 97 / 110 ms | 103 / 112 ms | 159 to 160 M |
| 10,000 | 361 / 381 ms | 375 / 397 ms | 360 / 379 ms | 628 to 634 M |
| 50,000 | **gas exhausted** after 836 / 969 ms | **exhausted** after 800 / 853 ms | **exhausted** after 810 / 876 ms | 1,000 M (the limit) |
| 200,000 | **exhausted** after 1,423 / 1,514 ms | **exhausted** after 1,419 / 1,566 ms | **exhausted** after 1,360 / 1,493 ms | 1,000 M (the limit) |

The scan costs about 63 to 80 k gas per message, so it reaches the 1e9 limit
somewhere between about 12,500 and 16,000 messages. Past that point it returns no answer at all.
At 10,000 messages, the indexed view is about 100 times faster in wall time
(3.6 ms against 375 ms at p50, common term) and uses about 460 times less gas
(1.36 M against 634 M). The in-repo baseline is `search-chat`'s own scan: the
mero-chat contract lives in another repository (mero-chat-pwa), which is not
checked out here, so its `search_messages` page was not run.

### Engine: size on disk, relative to raw text

From `results/engine.md`, index with words plus trigrams (the default, needed
for substring search), 1,000-document commits:

| messages | raw text | build | RocksDirectory live bytes | on disk (RocksDB dir) | on disk / raw text | MmapDirectory on disk | Mmap / raw |
|---|---|---|---|---|---|---|---|
| 2,000 | 0.13 MiB | 0.15 s | 0.59 MiB | 0.88 MiB | 6.9× | 0.59 MiB | 4.6× |
| 10,000 | 0.64 MiB | 0.92 s | 2.45 MiB | 2.75 MiB | 4.3× | 2.45 MiB | 3.8× |
| 50,000 | 3.19 MiB | 4.63 s | 11.34 MiB | 11.69 MiB | 3.7× | 11.31 MiB | 3.5× |
| 200,000 | 12.8 MiB | 19.21 s | 42.46 MiB | 85.18 MiB | 6.7× | 42.31 MiB | 3.3× |

With words only (no substring search), on disk is 1.6 to 2.0 times the raw text
at 50,000 to 200,000 messages. The live index is about 3.3 to 3.8 times the raw
text with trigrams. The RocksDB directory at 200,000 holds about twice its live
bytes because the chunks of merged-away segments are deleted but not yet
compacted. A real design would need compaction or a TTL on that column.

### Engine: raw query latency (no WASM, warm, single thread)

RocksDirectory, p50 / p95 over 400 queries:

| query | 2,000 | 10,000 | 50,000 | 200,000 |
|---|---|---|---|---|
| rare word | 7.6 / 54.9 µs | 9.9 / 60.3 µs | 52.5 / 95.2 µs | 298 / 498 µs |
| no match | 6.2 / 6.6 µs | 6.9 / 7.6 µs | 10.5 / 13.0 µs | 12.9 / 14.8 µs |
| common word, top 20 | 222 / 385 µs | 283 / 731 µs | 501 / 656 µs | 1.38 / 3.26 ms |
| prefix (4 chars) | 231 / 334 µs | 299 / 360 µs | 577 / 645 µs | 0.91 / 1.14 ms |
| substring (4 chars) | 53 / 112 µs | 129 / 202 µs | 502 / 865 µs | 1.63 / 3.01 ms |
| fuzzy (distance 1) | 118 / 237 µs | 277 / 395 µs | 627 / 814 µs | 0.89 / 1.43 ms |

At p50, RocksDirectory runs within about 20% of tantivy's own `MmapDirectory` at
every size. Its tails are wider at 200,000 (common word p95 3.26 ms against
2.13 ms); see `results/engine.md`. On 124 sampled queries at every size, the index returned exactly the match
count a fold-aware scan found. A cold open (empty chunk cache) plus the first
query takes about 1 ms at 2,000 messages and 4 to 8 ms at 200,000. So end-to-end
query time is dominated by the wasm view (about 2 ms floor), not the index.

### Memory

- **Heap, from the counting allocator, 10,000-document index:** reader open
  2.4 MiB; peak while indexing 27.6 MiB (tantivy's 15 MB writer arena is the
  floor); writer closed 0.8 MiB; after every query kind 3.5 MiB, of which the
  chunk cache is 2.3 MiB. With 10 contexts of 10,000 documents each open for
  query in a fresh service: 1.59 MiB per context.
- **Peak RSS of the whole end-to-end process** (two contexts, the wasm engine,
  RocksDB, the index): 182 MB at 2,000, 237 MB at 10,000, 570 MB at 50,000 and
  950 MB at 200,000. These totals include RocksDB's block cache and the
  seeding itself, so they are an upper bound and not the cost of the index.
- **Peak RSS of the engine harness** across all four sizes and both directories:
  594 MB.
- RocksDB's C++ block cache is not counted in the heap numbers, and it caches the
  same chunks a second time.

### Crash replay

The engine harness writes 5,000 posts, 500 edits and 500 deletes, crashes
mid-pass, and reopens the store. The replay picks up from the commit payload in
297 ms, and the documents, edits and deletes all match. Re-delivering an
already-indexed row changes nothing.

## Known limits and open questions

- **Rebuild on node restart: not needed.** The index is in the store, and a
  restart resumes from the commit payload. The dirty-row seq, however, is
  wall-clock nanoseconds, kept strictly increasing within one process. If the
  clock goes backwards across a restart, new rows could sort below the committed
  seq and be skipped. A real design needs a persisted counter.
- **App upgrade and schema changes.** Any change to the app's `SearchIndexSchema`
  (fields or `version`), the tokenizer version or the schema options wipes the
  index and rebuilds it from a scan (`a_schema_bump_rebuilds_from_a_scan`). The
  rebuild costs about 96 to 128 µs per message through wasm (4.8 s at 50,000
  messages, 25.6 s at 200,000), and the index serves nothing during it. There is no dual-index swap.
  A migration that rewrites every entity also produces one huge dirty
  row.
- **Snapshot sync is not covered.** A node bootstrapped from, or repaired by, a
  snapshot or a HashComparison or level-wise repair writes state without going
  through `internal_execute`, so no dirty row is staged. Its index stays stale
  until something forces a rebuild. This is the biggest gap. Either the sync apply
  paths must emit dirty rows, or the node must force a full build after any
  snapshot install. Neither is in the PoC.
- **Cold module cache.** The host handle is gated on the lock-selection
  `is_read_only_call`, which is `false` when the context's module is not yet
  cached (the first call after node start). A search view as the very first call
  would then trap. This was found by reading the code and has not been
  reproduced. The fix is to gate on the authoritative read-only set, as the
  delegated-read check a few lines later already does.
- **Fuzzy and typo tolerance** is tantivy's Levenshtein distance 1 (or 2) on
  words. It has no phonetic matching, no synonyms and no stemming. Words fold
  accents and case, CJK text becomes bigrams, and substring search needs at least
  3 characters, served by trigrams.
- **Memory with many contexts.** Readers are about 1.6 MiB per open context. An
  open writer costs a 15 MB arena, so the indexer must close writers between
  bursts, which `close_writers` does. There is no eviction of idle readers yet,
  so 1,000 active contexts would hold about 1.6 GB.
- **Host-side work is not metered.** A search is bounded by per-call limits and
  32 calls per execution, not by gas. A production version should charge gas in
  proportion to postings scanned, or at least a fixed cost per call.
- **Scores are per context.** BM25 IDF is computed inside each context's index,
  so scores do not compare across contexts. That follows from the isolation, and
  it matters only if some future feature merges results across contexts.
- **Per-user visibility inside one context** is left to the app's view (see
  "Isolation guarantees and their tests"). Delegated reads (`read_as`) were not
  exercised with search.
- **The extraction protocol is hand-written.** Apps write
  `search_poc_schema`/`_extract`/`_scan` as hex-borsh JSON views. A real design
  would have a `#[app::search_index]` macro generate them under the reserved
  `__calimero_` prefix, with the schema in the ABI.
- **The index can differ between nodes.** Every node indexes the state it holds,
  so two nodes that are not yet converged answer differently. That is expected,
  and the lag is bounded by the commit interval.

## Recommendation

**Take it to a real design.** The measurements support the approach:

- Search leaves the write path unchanged: gas is identical, and the only extra
  write is one 140-byte row in the same batch.
- A query costs 2 to 7 ms at p50 and at most about 1.4 M gas at every size
  measured, up to 200,000 messages. The in-WASM scan it replaces costs 375 ms and 634 M gas at 10,000
  messages, and cannot answer at all past about 12,500 to 16,000 messages.
- Indexing costs about 0.1 ms per message and stays off the write path. New
  messages become searchable within one 250 ms commit interval.
- The index takes 3.5 to 4 times the raw text with substring search, or about 2
  times without.
- The isolation properties hold by construction and are tested.

Before it ships, the design must close these, in order:

1. Dirty rows, or a forced rebuild, for snapshot and repair sync.
2. A persisted dirty-log seq.
3. The authoritative read-only gate for the host handle.
4. Gas for host-side search work.
5. The `#[app::search_index]` macro and an ABI-declared schema, replacing the
   hand-written views.
6. Compaction of the `SearchIndex` column, and eviction of idle readers.

The mero-chat `send_message` gas wall is a separate storage problem
(`AuthoredVector`), and search does not remove it.
