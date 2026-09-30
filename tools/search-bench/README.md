# search-bench: what full-text search costs on a node

Measurements of the node's per-context full-text search (`crates/search`), on the
final code of the branch that ships it. Two layers:

- **End to end** (`search_e2e_bench` and `insert_capacity` in
  `crates/context/src/handlers/execute/search_tests/bench.rs`): a live
  `ContextManager` on RocksDB running the real `search-chat` wasm. Seeding
  through `post_many`, indexing through the app's generated exports, queries
  through the app's `search` view (wasm, then `search_query`, then a read-back of
  each hit), and the in-WASM scan it replaces (`scan_search`).
- **Engine** (this crate, `results/engine.md`): `ContextIndex` and
  `SearchService` over `RocksDirectory` and a real RocksDB store, beside
  tantivy's stock `MmapDirectory`. No WASM. Index size, compaction, raw query
  latency, memory, freshness, crash replay, and the host-time fit behind search
  gas (`--only gas`).

How the feature works — opt-in, query API, consistency, isolation — is on the
docs site's *Search your app's data* page and in `crates/search/AGENTS.md`.

## Reproduce

```bash
PATH="$(scripts/setup-cargo-mero.sh):$PATH"
cargo mero build --manifest-path apps/search-chat/Cargo.toml
# The runs below used the release profile with LTO off, for build time:
export CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_OPT_LEVEL=3
cargo test --release -p calimero-context --lib insert_capacity -- --ignored --nocapture
SEARCH_BENCH_N=10000 cargo test --release -p calimero-context --lib search_e2e_bench -- --ignored --nocapture
cargo run --release -p search-bench -- --sizes 2000,10000,50000,200000 --out results/engine.md
```

Gas is a count of executed wasm operators plus what the host charges, so every
gas number here is deterministic and does not depend on the build profile or the
machine; wall times do.

**Machine:** Intel Xeon @ 2.80 GHz, 4 vCPUs, 15 GiB RAM, Linux 6.18 (a cloud
container, shared with another build while these ran). Corpus: synthetic chat
messages, Zipf-like over a 4,096-word vocabulary; ten six-letter words each, plus
`needle` in 1 in 200 and `zebrafish` in 1 in 2,000 (end to end); 11 words and 66
bytes on average (engine).

## How many inserts fit in one call

`post_many` of fresh messages into an empty map, one call, the default budget of
1,000 M gas; the batch size bisected until the next one fails. Measured with the
host gas charging for `search_query` disabled ("before") and enabled ("after"):

| | search | gas of 1 / 10 / 100 / 200 inserts | fixed per call | per insert | most inserts in one call | what stops the next |
|---|---|---|---|---|---|---|
| before | off | 2.29 M / 15.19 M / 152.73 M / 308.47 M | 0.73 M | 1.56 M | **636** (999.25 M gas) | gas exhausted |
| before | on | 2.29 M / 15.19 M / 152.73 M / 308.47 M | 0.73 M | 1.56 M | **636** (999.25 M gas) | gas exhausted |
| after | off | 2.29 M / 15.19 M / 152.73 M / 308.47 M | 0.73 M | 1.56 M | **636** (999.25 M gas) | gas exhausted |
| after | on | 2.29 M / 15.19 M / 152.73 M / 308.47 M | 0.73 M | 1.56 M | **636** (999.25 M gas) | gas exhausted |

"Per insert" is the slope from 100 to 200 inserts; "fixed" is the 1-insert call
minus one slope. The gas is linear: 0.73 M + 636 × 1.56 M = 993 M.

One `post` into a map that already holds N messages:

| messages already in the map | search off | search on (before) | search on (after) |
|---|---|---|---|
| 2,000 | 2.50 M | 2.50 M | 2.50 M |
| 10,000 | 2.55 M | 2.55 M | 2.55 M |
| 50,000 | 2.58 M | 2.58 M | 2.58 M |
| 200,000 | 2.61 M | 2.61 M | 2.61 M |

So an insert grows by about 4% from 2,000 to 200,000 messages (the child trie
the map's children live in is four levels deep, so a write touches the same rows
at any size), and search changes nothing, before or after gas charging. That is
by construction and has to stay so: the dirty row is written by the host after
the run, and a write can never call `search_query`, so the only work search adds
to a write is host bookkeeping outside gas. It must stay outside gas, because
every node has to agree whether a write ran out, and whether a node runs search
is its own choice.

## Index build (end to end, through the app's exports)

| messages | incremental drain of the seeding backlog | per message | full build from a scan of state | per message |
|---|---|---|---|---|
| 2,000 | 0.14 s | 69 µs | 0.16 s | 82 µs |
| 10,000 | 0.66 s | 66 µs | 0.64 s | 64 µs |
| 50,000 | 3.77 s | 75 µs | 3.69 s | 74 µs |
| 200,000 | 21.40 s | 107 µs | 22.32 s | 112 µs |

Almost all of it is extraction through wasm (47 to 102 µs a message); tantivy
takes 6 to 31 µs. A full build is what a snapshot install, a repair sync, a
migration or a schema bump costs; the previous commit keeps answering while it
runs.

## Write path: one `post`

| messages | search off p50 / p95 | search on p50 / p95 | gas off / on | indexing it later, off the write path |
|---|---|---|---|---|
| 2,000 | 5.13 / 9.87 ms | 4.97 / 9.64 ms | 2.50 M / 2.50 M | 151 µs |
| 10,000 | 5.30 / 9.72 ms | 5.40 / 9.63 ms | 2.53 M / 2.53 M | 329 µs |
| 50,000 | 4.83 / 9.03 ms | 5.03 / 9.14 ms | 2.59 M / 2.59 M | 192 µs |
| 200,000 | 6.53 / 13.03 ms | 6.82 / 14.11 ms | 2.67 M / 2.67 M | 309 µs |

The dirty row of one post names 3 entity ids: 205 bytes, plus a 40-byte counter
update, in the post's own write batch. Wall time is within run-to-run noise.
Freshness through the live indexer (250 ms commit interval, from the post
returning to a search finding it): p50 136 to 151 ms, p95 236 to 264 ms; max 259
to 262 ms up to 50,000 messages, and one outlier of 1.99 s at 200,000.

## Queries through the `search` view

Top 20, each hit read back from state; p50 / p95 and the gas of the whole view,
the host's search gas included:

| query | 2,000 | 10,000 | 50,000 | 200,000 |
|---|---|---|---|---|
| floor: `count` view, no search | 2.30 / 6.26 ms (0.09 M) | 2.00 / 4.00 ms (0.09 M) | 2.29 / 2.64 ms (0.09 M) | 2.38 / 4.75 ms (0.09 M) |
| rare word `zebrafish` | 2.13 / 5.56 ms (0.28 M), 2 hits | 2.47 / 6.42 ms (0.68 M), 6 | 4.06 / 6.90 ms (2.08 M), 26 | 3.73 / 6.56 ms (2.09 M), 101 |
| common word `kakaka` | 2.95 / 4.30 ms (2.03 M), 314 | 2.89 / 4.46 ms (2.10 M), 1,475 | 3.31 / 7.35 ms (2.27 M), 7,414 | 3.94 / 8.09 ms (2.82 M), 29,373 |
| no match `qqxqq` | 1.89 / 4.08 ms (0.08 M) | 2.04 / 2.42 ms (0.08 M) | 2.55 / 4.43 ms (0.08 M) | 2.28 / 5.01 ms (0.08 M) |
| prefix `needl` | 2.30 / 4.77 ms (0.99 M) | 3.01 / 5.92 ms (1.75 M) | 4.11 / 15.13 ms (1.76 M) | 3.71 / 8.49 ms (1.77 M) |
| substring `eedl` | 2.14 / 2.67 ms (0.99 M) | 2.73 / 6.47 ms (1.75 M) | 3.01 / 5.04 ms (1.75 M) | 3.99 / 9.20 ms (1.77 M) |
| fuzzy `neadle` | 2.25 / 3.75 ms (0.99 M) | 2.94 / 7.17 ms (1.75 M) | 2.99 / 4.13 ms (1.76 M) | 4.11 / 8.40 ms (1.77 M) |

A view's gas is bounded by its page, not the corpus: at most 2.82 M here, of
which the search itself (`8,000 + 25·matched + 18,500·hits + 125·bytes`) is
about 1.4 M for the common word at 200,000 messages.

### Baseline: the in-WASM scan (`scan_search`)

| messages | p50 / p95 | gas |
|---|---|---|
| 2,000 | 78.6 to 99.6 / 89.7 to 118.6 ms | 159 to 160 M |
| 10,000 | 343.7 to 363.5 / 369.3 to 406.1 ms | 628 to 634 M |
| 50,000 | **gas exhausted** after 658 to 673 ms | 1,000 M |
| 200,000 | **gas exhausted** after 853 to 907 ms | 1,000 M |

About 63,000 gas a message, so the scan stops answering before 16,000 messages.
At 10,000 the indexed view is about 120 times faster (2.9 against 344 ms at p50,
common word) and 300 times cheaper in gas (2.10 M against 634 M).

## Search gas

`search_query` charges `SEARCH_BASE_GAS + SEARCH_GAS_PER_MATCH·matched +
SEARCH_GAS_PER_HIT·hits + SEARCH_GAS_PER_BYTE·response bytes`
(`crates/runtime/src/logic/host_functions/search.rs`). The engine's `gas` section
times the host side of a call (the query and its borsh response) against the
work it reports, over 2,608 queries of every mode at every size, and fits it by
least squares:

| run | per call | per matched document | per hit | per response byte |
|---|---|---|---|---|
| first run (the constants are set from it) | 4.4 µs | 13.9 ns | 10.51 µs | 70.30 ns |
| rerun, the numbers in `results/engine.md` | 7.3 µs | 17.1 ns | 13.32 µs | 26.84 ns |

Hits and response bytes move together (a hit carries its snippet), so the fit
trades one against the other between runs; for the costly common-word query at
200,000 documents (64,576 matched, 20 hits, 2,188 bytes) the two fits predict
1.27 ms and 1.44 ms. The guest runs about 1.76 gas per ns (the scan baseline:
634 M gas in 360 ms; 2.0 per ns on this run), so the constants are 8,000 gas a
call, 25 a match, 18,500 a hit and 125 a byte.

## Engine: size on disk, and what compaction gives back

Words plus trigrams (the default, needed for substring search), 1,000-document
commits, RocksDirectory; `results/engine.md` has words-only and `MmapDirectory`
beside it:

| messages | raw text | build | live index bytes | on disk | after compacting the column | on disk / raw text |
|---|---|---|---|---|---|---|
| 2,000 | 0.13 MiB | 0.13 s | 0.59 MiB | 0.78 MiB | 0.79 MiB | 6.1× |
| 10,000 | 0.64 MiB | 0.66 s | 2.45 MiB | 2.44 MiB | 2.45 MiB | 3.8× |
| 50,000 | 3.19 MiB | 3.44 s | 11.33 MiB | 10.72 MiB | 10.73 MiB | 3.4× |
| 200,000 | 12.8 MiB | 14.93 s | 42.37 MiB | 68.76 MiB | **33.48 MiB** | 5.4× → 2.6× |

At 200,000 the column held twice its live bytes: tantivy's merges delete files
whose chunks stay behind as tombstones until a compaction reaches them. The
service tallies deleted bytes per context and compacts that context's slice of
the column once 64 MiB accumulate (`[context.search] compact_after_mib`).

## Engine: raw query latency (no WASM, warm, single thread)

RocksDirectory, p50 / p95 over 400 queries:

| query | 2,000 | 10,000 | 50,000 | 200,000 |
|---|---|---|---|---|
| rare word | 4.4 / 40.2 µs | 6.1 / 40.2 µs | 38.6 / 67.4 µs | 200 / 311 µs |
| no match | 3.7 / 3.8 µs | 4.2 / 4.3 µs | 8.0 / 8.4 µs | 9.9 / 11.3 µs |
| common word, top 20 | 154 / 195 µs | 188 / 227 µs | 317 / 413 µs | 747 µs / 1.45 ms |
| prefix (4 chars) | 145 / 180 µs | 181 / 215 µs | 320 / 355 µs | 516 / 841 µs |
| substring (4 chars) | 26.5 / 38.6 µs | 72.8 / 117 µs | 271 / 469 µs | 909 µs / 1.62 ms |
| fuzzy (distance 1) | 59.6 / 151 µs | 152 / 211 µs | 373 / 471 µs | 535 / 951 µs |

On 124 sampled queries at every size the index returned exactly the match count
a fold-aware scan found. A cold open plus the first query takes about 0.8 ms at
2,000 documents and 4 to 7 ms at 200,000.

## Memory

- Heap, 10,000-document index: reader open 2.4 MiB; peak while indexing
  27.5 MiB (tantivy's 15 MB writer arena is the floor); writer closed 0.8 MiB;
  after every query kind 3.5 MiB. Ten contexts of 10,000 documents open for
  query: 1.57 MiB each. With the default `max_open_indexes = 256`, open readers
  stay under about 400 MiB; idle ones close after 10 minutes and idle writers
  after 5 seconds.
- Peak RSS of the whole end-to-end process (two contexts, the wasm engine,
  RocksDB, the index): 196 MB at 2,000 messages, 264 MB at 10,000, 587 MB at
  50,000 and 1.27 GB at 200,000 — the seeding included, so an upper bound.
- Peak RSS of the engine run, every section: 698 MB.

## Crash replay

5,000 posts, 500 edits and 500 deletes, one dirty row each (141 bytes on
average); the first indexer pass dies mid-way, the store is reopened, and the
second pass resumes from the commit payload in 245 ms: documents, edits and
deletes all match, and a re-delivered row changes nothing.
