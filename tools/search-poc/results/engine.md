## Engine benchmark (tools/search-poc)

Release build, one thread per query unless stated, on this machine (4 cores). Per-message times are wall-clock CPU of the single indexing thread (tantivy's merge thread runs beside it).

### 2000 chat messages

Corpus: 2000 messages, 11.0 words and 66 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | store rows written | bytes written | live index bytes | on disk | per message on disk | on disk / raw text |
|---|---|---|---|---|---|---|---|---|---|
| words | Rocks | 0.10 s | 52.4 µs | 70 | 0.28 MiB | 0.28 MiB | 0.62 MiB | 326 B | 4.9× |
| words | Mmap | 0.12 s | 60.2 µs | — | — | — | 0.27 MiB | 143 B | 2.2× |
| words + trigrams | Rocks | 0.15 s | 76.3 µs | 74 | 0.60 MiB | 0.59 MiB | 0.88 MiB | 459 B | 6.9× |
| words + trigrams | Mmap | 0.17 s | 86.1 µs | — | — | — | 0.59 MiB | 309 B | 4.6× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RocksDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 0 | 7.6 µs | 54.9 µs | 63.9 µs | 7.7 µs | 53.4 µs | 61.7 µs |
| no match | 0 | 6.2 µs | 6.6 µs | 10.8 µs | 6.3 µs | 6.6 µs | 24.5 µs |
| common word, top-20 | 631 | 222 µs | 385 µs | 1281 µs | 219 µs | 270 µs | 373 µs |
| prefix (4 chars) | 41 | 231 µs | 334 µs | 411 µs | 225 µs | 420 µs | 2031 µs |
| infix substring (4 chars) | 13 | 53.0 µs | 112 µs | 200 µs | 47.4 µs | 91.7 µs | 161 µs |
| infix substring (3 chars) | 281 | 48.1 µs | 80.8 µs | 130 µs | 46.9 µs | 81.9 µs | 136 µs |
| two-term AND | 1 | 23.7 µs | 63.5 µs | 115 µs | 20.4 µs | 54.4 µs | 89.8 µs |
| sender facet + word | 57 | 215 µs | 304 µs | 438 µs | 214 µs | 296 µs | 452 µs |
| fuzzy (distance 1) | 5 | 118 µs | 237 µs | 274 µs | 117 µs | 241 µs | 275 µs |

Cold start (RocksDirectory, empty chunk cache): `zebrafish`: open 684 µs + first query 339 µs (16 chunk reads, 65 cache hits); `needle`: open 459 µs + first query 347 µs (16 chunk reads, 70 cache hits); `kaka`: open 351 µs + first query 465 µs (16 chunk reads, 70 cache hits).

Single message + its own commit at 2000 docs (RocksDirectory): p50 11.5 ms, p99 34.9 ms — what batching over ~250 ms avoids.

Native linear scan over 2000 in-memory strings (lowercase + contains + sort, the mero-chat `search_all_messages` semantics, a LOWER bound — no WASM, no storage reads): rare p50 319 µs / p99 2217 µs, common p50 356 µs / p99 2521 µs, infix p50 312 µs / p99 442 µs.


### 10000 chat messages

Corpus: 10000 messages, 11.1 words and 67 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | store rows written | bytes written | live index bytes | on disk | per message on disk | on disk / raw text |
|---|---|---|---|---|---|---|---|---|---|
| words | Rocks | 0.59 s | 59.0 µs | 372 | 2.37 MiB | 1.17 MiB | 1.46 MiB | 153 B | 2.3× |
| words | Mmap | 0.75 s | 75.1 µs | — | — | — | 1.17 MiB | 122 B | 1.8× |
| words + trigrams | Rocks | 0.92 s | 91.9 µs | 407 | 4.92 MiB | 2.45 MiB | 2.75 MiB | 288 B | 4.3× |
| words + trigrams | Mmap | 0.89 s | 89.2 µs | — | — | — | 2.45 MiB | 256 B | 3.8× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RocksDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 1 | 9.9 µs | 60.3 µs | 120 µs | 9.7 µs | 57.4 µs | 64.6 µs |
| no match | 0 | 6.9 µs | 7.6 µs | 11.5 µs | 6.9 µs | 7.1 µs | 11.0 µs |
| common word, top-20 | 3186 | 283 µs | 731 µs | 2453 µs | 277 µs | 558 µs | 2044 µs |
| prefix (4 chars) | 209 | 299 µs | 360 µs | 536 µs | 277 µs | 336 µs | 508 µs |
| infix substring (4 chars) | 68 | 129 µs | 202 µs | 434 µs | 115 µs | 187 µs | 244 µs |
| infix substring (3 chars) | 1395 | 82.2 µs | 252 µs | 911 µs | 83.1 µs | 181 µs | 254 µs |
| two-term AND | 4 | 47.0 µs | 243 µs | 292 µs | 40.7 µs | 235 µs | 274 µs |
| sender facet + word | 278 | 287 µs | 374 µs | 463 µs | 287 µs | 373 µs | 1298 µs |
| fuzzy (distance 1) | 22 | 277 µs | 395 µs | 736 µs | 282 µs | 429 µs | 968 µs |

Cold start (RocksDirectory, empty chunk cache): `zebrafish`: open 986 µs + first query 912 µs (30 chunk reads, 85 cache hits); `needle`: open 702 µs + first query 938 µs (32 chunk reads, 101 cache hits); `kaka`: open 913 µs + first query 1127 µs (32 chunk reads, 101 cache hits).

Single message + its own commit at 10000 docs (RocksDirectory): p50 21.7 ms, p99 31.2 ms — what batching over ~250 ms avoids.

Native linear scan over 10000 in-memory strings (lowercase + contains + sort, the mero-chat `search_all_messages` semantics, a LOWER bound — no WASM, no storage reads): rare p50 1741 µs / p99 4198 µs, common p50 2014 µs / p99 4300 µs, infix p50 1696 µs / p99 3330 µs.


### 50000 chat messages

Corpus: 50000 messages, 11.1 words and 67 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | store rows written | bytes written | live index bytes | on disk | per message on disk | on disk / raw text |
|---|---|---|---|---|---|---|---|---|---|
| words | Rocks | 2.81 s | 56.2 µs | 1924 | 15.86 MiB | 5.45 MiB | 5.79 MiB | 121 B | 1.8× |
| words | Mmap | 3.62 s | 72.4 µs | — | — | — | 5.43 MiB | 113 B | 1.7× |
| words + trigrams | Rocks | 4.63 s | 92.6 µs | 2162 | 32.42 MiB | 11.34 MiB | 11.69 MiB | 245 B | 3.7× |
| words + trigrams | Mmap | 4.88 s | 97.6 µs | — | — | — | 11.31 MiB | 237 B | 3.5× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RocksDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 3 | 52.5 µs | 95.2 µs | 107 µs | 49.9 µs | 91.7 µs | 99.1 µs |
| no match | 0 | 10.5 µs | 13.0 µs | 30.7 µs | 10.6 µs | 11.4 µs | 31.7 µs |
| common word, top-20 | 15951 | 501 µs | 656 µs | 698 µs | 489 µs | 649 µs | 769 µs |
| prefix (4 chars) | 1058 | 577 µs | 645 µs | 722 µs | 500 µs | 554 µs | 669 µs |
| infix substring (4 chars) | 339 | 502 µs | 865 µs | 924 µs | 456 µs | 806 µs | 855 µs |
| infix substring (3 chars) | 6926 | 300 µs | 601 µs | 2470 µs | 270 µs | 537 µs | 743 µs |
| two-term AND | 23 | 203 µs | 393 µs | 429 µs | 184 µs | 358 µs | 423 µs |
| sender facet + word | 1403 | 635 µs | 1366 µs | 2927 µs | 550 µs | 999 µs | 1322 µs |
| fuzzy (distance 1) | 115 | 627 µs | 814 µs | 1475 µs | 651 µs | 926 µs | 1680 µs |

Cold start (RocksDirectory, empty chunk cache): `zebrafish`: open 1881 µs + first query 1209 µs (75 chunk reads, 206 cache hits); `needle`: open 1670 µs + first query 1887 µs (86 chunk reads, 221 cache hits); `kaka`: open 1696 µs + first query 2846 µs (85 chunk reads, 224 cache hits).

Single message + its own commit at 50000 docs (RocksDirectory): p50 13.4 ms, p99 22.6 ms — what batching over ~250 ms avoids.

Native linear scan over 50000 in-memory strings (lowercase + contains + sort, the mero-chat `search_all_messages` semantics, a LOWER bound — no WASM, no storage reads): rare p50 9530 µs / p99 13.9 ms, common p50 11.0 ms / p99 22.1 ms, infix p50 9480 µs / p99 16.5 ms.


### 200000 chat messages

Corpus: 200000 messages, 11.1 words and 67 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | store rows written | bytes written | live index bytes | on disk | per message on disk | on disk / raw text |
|---|---|---|---|---|---|---|---|---|---|
| words | Rocks | 12.26 s | 61.3 µs | 7997 | 79.35 MiB | 20.29 MiB | 25.06 MiB | 131 B | 2.0× |
| words | Mmap | 12.27 s | 61.4 µs | — | — | — | 20.23 MiB | 106 B | 1.6× |
| words + trigrams | Rocks | 19.21 s | 96.0 µs | 9174 | 160.19 MiB | 42.46 MiB | 85.18 MiB | 446 B | 6.7× |
| words + trigrams | Mmap | 20.15 s | 101 µs | — | — | — | 42.31 MiB | 221 B | 3.3× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RocksDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 11 | 298 µs | 498 µs | 584 µs | 271 µs | 445 µs | 497 µs |
| no match | 0 | 12.9 µs | 14.8 µs | 31.5 µs | 13.0 µs | 14.1 µs | 32.0 µs |
| common word, top-20 | 64088 | 1376 µs | 3261 µs | 5448 µs | 1325 µs | 2128 µs | 3512 µs |
| prefix (4 chars) | 4239 | 907 µs | 1136 µs | 2204 µs | 762 µs | 932 µs | 996 µs |
| infix substring (4 chars) | 1366 | 1633 µs | 3011 µs | 3266 µs | 1592 µs | 2935 µs | 3111 µs |
| infix substring (3 chars) | 27812 | 769 µs | 1981 µs | 3003 µs | 771 µs | 1980 µs | 2967 µs |
| two-term AND | 89 | 691 µs | 994 µs | 2188 µs | 633 µs | 920 µs | 1920 µs |
| sender facet + word | 5685 | 1518 µs | 3179 µs | 4151 µs | 1412 µs | 3060 µs | 4365 µs |
| fuzzy (distance 1) | 466 | 887 µs | 1429 µs | 3226 µs | 845 µs | 1065 µs | 1432 µs |

Cold start (RocksDirectory, empty chunk cache): `zebrafish`: open 4351 µs + first query 1566 µs (101 chunk reads, 279 cache hits); `needle`: open 2621 µs + first query 2658 µs (122 chunk reads, 293 cache hits); `kaka`: open 2592 µs + first query 5037 µs (124 chunk reads, 293 cache hits).

Single message + its own commit at 200000 docs (RocksDirectory): p50 14.2 ms, p99 25.3 ms — what batching over ~250 ms avoids.

Native linear scan over 200000 in-memory strings (lowercase + contains + sort, the mero-chat `search_all_messages` semantics, a LOWER bound — no WASM, no storage reads): rare p50 38.1 ms / p99 42.0 ms, common p50 42.2 ms / p99 54.2 ms, infix p50 37.3 ms / p99 44.7 ms.


### Memory (Rust heap, counting allocator)

| state | heap |
|---|---|
| one index open, reader only (10k docs to come) | 2.38 MiB |
| + writer open (1 thread, 15 MB arena budget) | 2.39 MiB |
| peak while indexing 10k docs in 1k-doc commits | 27.58 MiB |
| writer closed | 0.79 MiB |
| after every query kind ran (chunk cache holds 2.31 MiB) | 3.52 MiB |
| 10 contexts × 10k docs open for query, fresh service (per context) | 15.86 MiB (1.59 MiB each) |

RocksDB's own block cache (the node's DEFAULT_BLOCK_CACHE_SIZE) is C++-allocated and not counted; it caches the same chunks a second time.


### Cross-context: 10 contexts × 10k messages

Built in 7.3 s.

| query | sequential p50 | p99 | parallel (10 threads) + merge p50 | p99 |
|---|---|---|---|---|
| rare word | 386 µs | 1515 µs | 1343 µs | 7811 µs |
| no match | 160 µs | 340 µs | 1288 µs | 5270 µs |
| common word, top-20 | 3866 µs | 9934 µs | 3009 µs | 9962 µs |
| prefix (4 chars) | 4481 µs | 10.0 ms | 3291 µs | 5763 µs |
| infix substring (4 chars) | 2305 µs | 8474 µs | 1985 µs | 7396 µs |
| two-term AND | 1143 µs | 6547 µs | 1733 µs | 8094 µs |
| sender facet + word | 4164 µs | 9222 µs | 3032 µs | 8360 µs |
| fuzzy (distance 1) | 4444 µs | 12.0 ms | 2840 µs | 10.4 ms |

The merge sorts by raw BM25, which is only roughly comparable across contexts (each has its own IDF — by design, see isolation). Threads here are spawned per query; a node would use a pool.


### Freshness and crash-restart replay (SearchService, RocksDB)

Apply → searchable with a 250 ms commit interval (40 writes at random tick phases, 10k-doc index): p50 138.8 ms, p99 258.5 ms, max 258.5 ms.
Apply → searchable with a 50 ms commit interval (40 writes at random tick phases, 10k-doc index): p50 39.4 ms, p99 60.2 ms, max 60.2 ms.

Replay: 5000 posts + 500 edits + 500 deletes, one dirty row each (76 B per row on average). Run 1 crashed mid-pass (simulated crash during extraction); it had committed through seq 1790700129563466191 and left 5000 dirty rows.
Run 2 (after reopening the store) resumed from the commit payload: 5000 rows, 5000 ids, 4100 docs in 297.1 ms → documents OK (4500 vs 4500 expected), edits OK , deletes OK, dirty log empty true. A re-delivered, already-indexed row replays idempotently (count unchanged: true).


Peak RSS of the whole run (every section, RocksDB included): 593616 kB.

