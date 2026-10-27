# search-bench: what full-text search costs on a node

Engine measurements of per-app full-text search (`crates/search`):
`ContextIndex` over tantivy's `RamDirectory`, which is what the node's
`services::Search` (`crates/cloud-node`) opens every app's index over, beside
tantivy's stock `MmapDirectory` as an on-disk baseline. No WASM, no cluster:
index size, raw query latency, memory, many apps' indexes side by side, and the
host-time fit behind search gas (`--only gas`). The full report of the run below
is `results/engine.md`.

How the feature works (opt-in, query API, how the index follows state) is on the
docs site's *Full-text search* page and in `crates/search/AGENTS.md`. End to end
(the real `search-chat` wasm in a 3-node cluster) it is tested, not benched, by
`views_search_the_apps_own_index_as_it_changes` in `crates/cloud-node/tests/cluster.rs`.

## Reproduce

```bash
# The run below used the release profile with LTO off, for build time:
export CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_OPT_LEVEL=3
cargo run --release -p search-bench -- --sizes 2000,10000,50000,200000 --docs --out tools/search-bench/results/engine.md
```

`--only size|memory|cross|gas` runs one section; `--iters N` sets the timed
queries per row (default 400). CI runs it on master pushes and labelled PRs
(`.github/workflows/benchmarks.yml`, job `search-bench`) and uploads the report.

**Machine:** 4 vCPUs, Linux 6.18, a cloud container (wall times vary between
runs by 10% or more). Corpus: synthetic chat messages, Zipf-like over a
20,000-word vocabulary of pronounceable pseudo-words, 30% of them a realistic
sentence (accents, CJK); 11 words and 66 bytes on average, `needle` in 1 in 200
and `zebrafish` in five.

## What changed since v1

v1 kept each context's index in its node's RocksDB (`RocksDirectory`, a chunk
cache, a dirty log replayed by an indexer). Release-v2 keeps it in memory and
rebuilds it from state on first use after a restart, so the store-backed
directory, the column compaction and the crash-replay section are gone with it.
The v1 numbers, and the end-to-end ones measured through v1's
`ContextManager`, are in this directory's history on `master`.

## Index size and build

Words plus trigrams (the default, needed for substring search), 1,000-document
commits; `results/engine.md` has words-only beside it:

| messages | raw text | build (Ram) | per message | index in memory | index / raw text | build (Mmap) |
|---|---|---|---|---|---|---|
| 2,000 | 0.13 MiB | 0.04 s | 21 µs | 0.59 MiB | 4.6× | 0.05 s |
| 10,000 | 0.64 MiB | 0.24 s | 25 µs | 2.45 MiB | 3.8× | 0.32 s |
| 50,000 | 3.19 MiB | 1.29 s | 26 µs | 11.31 MiB | 3.5× | 2.00 s |
| 200,000 | 12.8 MiB | 5.72 s | 29 µs | 42.40 MiB | 3.3× | 7.44 s |

The build is what the first view after a restart (or after a schema change)
pays for the app's index, on top of extracting each document through the app's
wasm. The index is the same size in either directory; v1's RocksDB column held
up to twice its live bytes until compaction reached the chunks merges deleted.

## Raw query latency (no WASM, warm, single thread)

RamDirectory, p50 / p95 over 400 queries:

| query | 2,000 | 10,000 | 50,000 | 200,000 |
|---|---|---|---|---|
| rare word | 2.8 / 21.0 µs | 3.7 / 21.0 µs | 22.3 / 39.0 µs | 101 / 172 µs |
| no match | 2.2 / 2.4 µs | 2.4 / 2.5 µs | 3.7 / 3.8 µs | 4.4 / 4.7 µs |
| common word, top 20 | 74.5 / 94.3 µs | 91.6 / 112 µs | 171 / 229 µs | 406 / 577 µs |
| prefix (4 chars) | 104 / 125 µs | 161 / 187 µs | 274 / 314 µs | 382 / 432 µs |
| substring (4 chars) | 14.4 / 21.4 µs | 39.0 / 62.7 µs | 154 / 276 µs | 495 / 915 µs |
| fuzzy (distance 1) | 32.7 / 59.3 µs | 77.0 / 97.7 µs | 193 / 235 µs | 262 / 335 µs |

MmapDirectory is within noise of it (the OS page cache holds the files), and on
124 sampled queries at every size the index returned exactly the match count a
fold-aware scan found. The baseline an app without search has, a native
lowercase-and-`contains` scan over the same strings in memory (a lower bound:
no WASM, no state reads), takes 0.09 ms at 2,000 messages, 0.59 ms at 10,000,
2.9 ms at 50,000 and 12 to 15 ms at 200,000.

## Catching up on one write

A view brings its app's index up to the state it reads: it applies the entities
the entries since the last view touched, then commits. One message applied and
committed on its own takes **3.2 to 3.8 ms** at p50 at every size (p99 6 to
105 ms, when the commit waits for a merge): the commit, not the document,
dominates, so a view after a burst of writes pays about the same as after one.

## Memory

Rust heap, counting allocator, the `RamDirectory`'s bytes included:

- An empty index open: 2.4 MiB. Peak while indexing 10,000 documents in
  1,000-document commits: 28.5 MiB (tantivy's 15 MB writer arena is the floor);
  writer closed: 3.5 MiB, of which the directory holds 2.4 MiB.
- Ten apps' indexes of 10,000 documents each, open for query: **3.0 MiB each**.
  The node keeps every searchable app's index resident, so this is the memory
  cost per app at this size; at 200,000 messages the index alone is 42 MiB.
- Docs corpus (10,000 documents × 50 blocks, one search document per block,
  107 MiB of text): built in 35.6 s, 291 MiB in memory.
- Peak RSS of the whole engine run, every section: 839 MiB.

## Search gas

`search_query` charges `SEARCH_BASE_GAS + SEARCH_GAS_PER_MATCH·matched +
SEARCH_GAS_PER_HIT·hits + SEARCH_GAS_PER_BYTE·response bytes`
(`crates/runtime/src/logic/host_functions/search.rs`). The `gas` section times
the host side of a call (the query and its borsh response) against the work it
reports, over 2,608 queries of every mode at every size, and fits it by least
squares:

| run | per call | per matched document | per hit | per response byte |
|---|---|---|---|---|
| v1 first run (the constants are set from it) | 4.4 µs | 13.9 ns | 10.51 µs | 70.30 ns |
| this run, `results/engine.md` | 0.1 µs | 5.8 ns | 5.72 µs | 33.27 ns |

Every coefficient is lower in memory than over v1's RocksDB directory, so the
constants (8,000 gas a call, 25 a match, 18,500 a hit and 125 a byte, at the
guest's ~1.76 gas per ns) still over-charge the host's time; hits and response
bytes move together (a hit carries its snippet), so the fit trades one against
the other between runs.
