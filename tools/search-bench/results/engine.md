## Engine benchmark (tools/search-bench)

Release build, one thread per query unless stated, on this machine (4 cores). Per-message times are wall-clock CPU of the single indexing thread (tantivy's merge thread runs beside it).

### 2000 chat messages

Corpus: 2000 messages, 11.0 words and 66 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | store rows written | bytes written | live index bytes | on disk | after compacting the column | per message on disk | on disk / raw text |
|---|---|---|---|---|---|---|---|---|---|---|
| words | Rocks | 0.11 s | 53.9 µs | 70 | 0.28 MiB | 0.28 MiB | 0.58 MiB | 0.59 MiB | 305 B | 4.6× |
| words | Mmap | 0.18 s | 90.8 µs | — | — | — | 0.27 MiB | — | 143 B | 2.2× |
| words + trigrams | Rocks | 0.13 s | 66.3 µs | 74 | 0.60 MiB | 0.59 MiB | 0.78 MiB | 0.79 MiB | 408 B | 6.1× |
| words + trigrams | Mmap | 0.22 s | 111 µs | — | — | — | 0.59 MiB | — | 309 B | 4.6× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RocksDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 0 | 4.4 µs | 40.2 µs | 41.9 µs | 4.4 µs | 32.4 µs | 39.8 µs |
| no match | 0 | 3.7 µs | 3.8 µs | 8.4 µs | 3.6 µs | 3.7 µs | 7.4 µs |
| common word, top-20 | 631 | 154 µs | 195 µs | 4219 µs | 151 µs | 186 µs | 204 µs |
| prefix (4 chars) | 41 | 145 µs | 180 µs | 200 µs | 140 µs | 175 µs | 202 µs |
| infix substring (4 chars) | 13 | 26.5 µs | 38.6 µs | 59.9 µs | 23.5 µs | 36.3 µs | 54.6 µs |
| infix substring (3 chars) | 281 | 26.8 µs | 36.0 µs | 52.5 µs | 25.6 µs | 34.6 µs | 54.9 µs |
| two-term AND | 1 | 13.6 µs | 36.3 µs | 52.4 µs | 11.2 µs | 33.5 µs | 49.3 µs |
| sender facet + word | 57 | 144 µs | 184 µs | 204 µs | 141 µs | 182 µs | 200 µs |
| fuzzy (distance 1) | 5 | 59.6 µs | 151 µs | 173 µs | 59.4 µs | 152 µs | 193 µs |

Cold start (RocksDirectory, empty chunk cache): `zebrafish`: open 585 µs + first query 223 µs (16 chunk reads, 65 cache hits); `needle`: open 352 µs + first query 259 µs (16 chunk reads, 70 cache hits); `kaka`: open 292 µs + first query 343 µs (16 chunk reads, 68 cache hits).

Single message + its own commit at 2000 docs (RocksDirectory): p50 10.6 ms, p99 23.3 ms — what batching over ~250 ms avoids.

Native linear scan over 2000 in-memory strings (lowercase + contains + sort, the mero-chat `search_all_messages` semantics, a LOWER bound — no WASM, no storage reads): rare p50 184 µs / p99 243 µs, common p50 215 µs / p99 351 µs, infix p50 187 µs / p99 229 µs.


### 10000 chat messages

Corpus: 10000 messages, 11.1 words and 67 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | store rows written | bytes written | live index bytes | on disk | after compacting the column | per message on disk | on disk / raw text |
|---|---|---|---|---|---|---|---|---|---|---|
| words | Rocks | 0.41 s | 41.0 µs | 372 | 2.37 MiB | 1.17 MiB | 1.32 MiB | 1.33 MiB | 138 B | 2.1× |
| words | Mmap | 0.99 s | 98.7 µs | — | — | — | 1.17 MiB | — | 122 B | 1.8× |
| words + trigrams | Rocks | 0.66 s | 66.2 µs | 407 | 4.92 MiB | 2.45 MiB | 2.44 MiB | 2.45 MiB | 255 B | 3.8× |
| words + trigrams | Mmap | 0.79 s | 78.8 µs | — | — | — | 2.45 MiB | — | 256 B | 3.8× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RocksDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 1 | 6.1 µs | 40.2 µs | 58.1 µs | 6.1 µs | 39.1 µs | 40.4 µs |
| no match | 0 | 4.2 µs | 4.3 µs | 5.0 µs | 4.2 µs | 4.3 µs | 8.3 µs |
| common word, top-20 | 3186 | 188 µs | 227 µs | 261 µs | 187 µs | 225 µs | 275 µs |
| prefix (4 chars) | 209 | 181 µs | 215 µs | 238 µs | 166 µs | 199 µs | 229 µs |
| infix substring (4 chars) | 68 | 72.8 µs | 117 µs | 152 µs | 66.2 µs | 110 µs | 133 µs |
| infix substring (3 chars) | 1395 | 42.9 µs | 78.9 µs | 101 µs | 44.3 µs | 75.4 µs | 94.5 µs |
| two-term AND | 4 | 32.5 µs | 166 µs | 215 µs | 25.7 µs | 161 µs | 167 µs |
| sender facet + word | 278 | 180 µs | 237 µs | 278 µs | 175 µs | 232 µs | 266 µs |
| fuzzy (distance 1) | 22 | 152 µs | 211 µs | 249 µs | 148 µs | 208 µs | 225 µs |

Cold start (RocksDirectory, empty chunk cache): `zebrafish`: open 776 µs + first query 470 µs (29 chunk reads, 86 cache hits); `needle`: open 459 µs + first query 977 µs (32 chunk reads, 103 cache hits); `kaka`: open 458 µs + first query 652 µs (31 chunk reads, 101 cache hits).

Single message + its own commit at 10000 docs (RocksDirectory): p50 19.1 ms, p99 42.7 ms — what batching over ~250 ms avoids.

Native linear scan over 10000 in-memory strings (lowercase + contains + sort, the mero-chat `search_all_messages` semantics, a LOWER bound — no WASM, no storage reads): rare p50 978 µs / p99 5103 µs, common p50 1162 µs / p99 1299 µs, infix p50 1004 µs / p99 1176 µs.


### 50000 chat messages

Corpus: 50000 messages, 11.1 words and 67 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | store rows written | bytes written | live index bytes | on disk | after compacting the column | per message on disk | on disk / raw text |
|---|---|---|---|---|---|---|---|---|---|---|
| words | Rocks | 2.09 s | 41.8 µs | 1924 | 15.81 MiB | 5.45 MiB | 5.21 MiB | 5.22 MiB | 109 B | 1.6× |
| words | Mmap | 3.82 s | 76.5 µs | — | — | — | 5.43 MiB | — | 113 B | 1.7× |
| words + trigrams | Rocks | 3.44 s | 68.8 µs | 2162 | 32.32 MiB | 11.33 MiB | 10.72 MiB | 10.73 MiB | 224 B | 3.4× |
| words + trigrams | Mmap | 5.41 s | 108 µs | — | — | — | 11.31 MiB | — | 237 B | 3.5× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RocksDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 3 | 38.6 µs | 67.4 µs | 84.4 µs | 35.6 µs | 64.5 µs | 83.4 µs |
| no match | 0 | 8.0 µs | 8.4 µs | 14.2 µs | 8.1 µs | 9.0 µs | 31.3 µs |
| common word, top-20 | 15951 | 317 µs | 413 µs | 447 µs | 321 µs | 405 µs | 439 µs |
| prefix (4 chars) | 1058 | 320 µs | 355 µs | 380 µs | 263 µs | 316 µs | 373 µs |
| infix substring (4 chars) | 339 | 271 µs | 469 µs | 507 µs | 247 µs | 432 µs | 454 µs |
| infix substring (3 chars) | 6926 | 171 µs | 295 µs | 335 µs | 155 µs | 287 µs | 321 µs |
| two-term AND | 23 | 122 µs | 239 µs | 280 µs | 109 µs | 228 µs | 255 µs |
| sender facet + word | 1403 | 306 µs | 514 µs | 553 µs | 284 µs | 503 µs | 567 µs |
| fuzzy (distance 1) | 115 | 373 µs | 471 µs | 529 µs | 355 µs | 434 µs | 452 µs |

Cold start (RocksDirectory, empty chunk cache): `zebrafish`: open 1727 µs + first query 994 µs (73 chunk reads, 206 cache hits); `needle`: open 1329 µs + first query 1362 µs (85 chunk reads, 221 cache hits); `kaka`: open 1236 µs + first query 1451 µs (86 chunk reads, 223 cache hits).

Single message + its own commit at 50000 docs (RocksDirectory): p50 12.1 ms, p99 39.4 ms — what batching over ~250 ms avoids.

Native linear scan over 50000 in-memory strings (lowercase + contains + sort, the mero-chat `search_all_messages` semantics, a LOWER bound — no WASM, no storage reads): rare p50 5553 µs / p99 10.4 ms, common p50 6537 µs / p99 11.3 ms, infix p50 5628 µs / p99 9604 µs.


### 200000 chat messages

Corpus: 200000 messages, 11.1 words and 67 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | store rows written | bytes written | live index bytes | on disk | after compacting the column | per message on disk | on disk / raw text |
|---|---|---|---|---|---|---|---|---|---|---|
| words | Rocks | 10.25 s | 51.3 µs | 7985 | 78.31 MiB | 20.26 MiB | 22.48 MiB | 14.44 MiB | 117 B | 1.8× |
| words | Mmap | 16.44 s | 82.2 µs | — | — | — | 20.23 MiB | — | 106 B | 1.6× |
| words + trigrams | Rocks | 14.93 s | 74.6 µs | 9172 | 159.45 MiB | 42.37 MiB | 68.76 MiB | 33.48 MiB | 360 B | 5.4× |
| words + trigrams | Mmap | 19.56 s | 97.8 µs | — | — | — | 42.29 MiB | — | 221 B | 3.3× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RocksDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 11 | 200 µs | 311 µs | 346 µs | 179 µs | 289 µs | 330 µs |
| no match | 0 | 9.9 µs | 11.3 µs | 33.9 µs | 10.0 µs | 11.2 µs | 32.4 µs |
| common word, top-20 | 64088 | 747 µs | 1445 µs | 1705 µs | 729 µs | 1247 µs | 1710 µs |
| prefix (4 chars) | 4239 | 516 µs | 841 µs | 1175 µs | 413 µs | 833 µs | 939 µs |
| infix substring (4 chars) | 1366 | 909 µs | 1621 µs | 1765 µs | 874 µs | 1597 µs | 1832 µs |
| infix substring (3 chars) | 27812 | 492 µs | 1056 µs | 1320 µs | 462 µs | 1038 µs | 1307 µs |
| two-term AND | 89 | 414 µs | 559 µs | 888 µs | 366 µs | 645 µs | 927 µs |
| sender facet + word | 5685 | 730 µs | 1672 µs | 2298 µs | 675 µs | 1608 µs | 2383 µs |
| fuzzy (distance 1) | 466 | 535 µs | 951 µs | 1223 µs | 513 µs | 854 µs | 1211 µs |

Cold start (RocksDirectory, empty chunk cache): `zebrafish`: open 3351 µs + first query 1714 µs (100 chunk reads, 277 cache hits); `needle`: open 2405 µs + first query 2276 µs (123 chunk reads, 290 cache hits); `kaka`: open 2246 µs + first query 3475 µs (123 chunk reads, 290 cache hits).

Single message + its own commit at 200000 docs (RocksDirectory): p50 12.2 ms, p99 30.8 ms — what batching over ~250 ms avoids.

Native linear scan over 200000 in-memory strings (lowercase + contains + sort, the mero-chat `search_all_messages` semantics, a LOWER bound — no WASM, no storage reads): rare p50 22.8 ms / p99 30.6 ms, common p50 27.6 ms / p99 29.0 ms, infix p50 23.5 ms / p99 28.4 ms.


### Memory (Rust heap, counting allocator)

| state | heap |
|---|---|
| one index open, reader only (10k docs to come) | 2.39 MiB |
| + writer open (1 thread, 15 MB arena budget) | 2.39 MiB |
| peak while indexing 10k docs in 1k-doc commits | 26.43 MiB |
| writer closed | 1.97 MiB |
| after every query kind ran (chunk cache holds 2.31 MiB) | 3.52 MiB |
| 10 contexts × 10k docs open for query, fresh service (per context) | 15.67 MiB (1.57 MiB each) |

RocksDB's own block cache (the node's DEFAULT_BLOCK_CACHE_SIZE) is C++-allocated and not counted; it caches the same chunks a second time.


### Cross-context: 10 contexts × 10k messages

Built in 4.8 s.

| query | sequential p50 | p99 | parallel (10 threads) + merge p50 | p99 |
|---|---|---|---|---|
| rare word | 255 µs | 935 µs | 3720 µs | 5491 µs |
| no match | 104 µs | 171 µs | 3921 µs | 13.9 ms |
| common word, top-20 | 2412 µs | 3788 µs | 4146 µs | 9747 µs |
| prefix (4 chars) | 2518 µs | 3207 µs | 4507 µs | 10.0 ms |
| infix substring (4 chars) | 1212 µs | 2832 µs | 2869 µs | 6983 µs |
| two-term AND | 696 µs | 2438 µs | 3290 µs | 6392 µs |
| sender facet + word | 2576 µs | 6192 µs | 4172 µs | 10.2 ms |
| fuzzy (distance 1) | 2229 µs | 2985 µs | 2463 µs | 6277 µs |

The merge sorts by raw BM25, which is only roughly comparable across contexts (each has its own IDF — by design, see isolation). Threads here are spawned per query; a node would use a pool.


### Freshness and crash-restart replay (SearchService, RocksDB)

Apply → searchable with a 250 ms commit interval (40 writes at random tick phases, 10k-doc index): p50 137.2 ms, p99 254.4 ms, max 254.4 ms.
Apply → searchable with a 50 ms commit interval (40 writes at random tick phases, 10k-doc index): p50 33.2 ms, p99 57.2 ms, max 57.2 ms.

Replay: 5000 posts + 500 edits + 500 deletes, one dirty row each (141 B per row on average). Run 1 crashed mid-pass (simulated crash during extraction); it had committed through seq 1000 and left 6000 dirty rows.
Run 2 (after reopening the store) resumed from the commit payload: 5000 rows, 5000 ids, 4100 docs in 245.2 ms → documents OK (4500 vs 4500 expected), edits OK , deletes OK, dirty log empty true. A re-delivered, already-indexed row replays idempotently (count unchanged: true).


### The host time of a query, for search gas

| messages | query | matched | hits | response bytes | host p50 |
|---|---|---|---|---|---|
| 2000 | rare word | 0 | 0 | 55 | 5.2 µs |
| 2000 | no match | 0 | 0 | 13 | 3.8 µs |
| 2000 | common word, top-20 | 643 | 20 | 2449 | 164 µs |
| 2000 | prefix (4 chars) | 44 | 19 | 796 | 150 µs |
| 2000 | infix substring (4 chars) | 14 | 12 | 503 | 28.4 µs |
| 2000 | infix substring (3 chars) | 273 | 20 | 817 | 26.6 µs |
| 2000 | two-term AND | 1 | 1 | 175 | 21.4 µs |
| 2000 | sender facet + word | 60 | 20 | 2423 | 158 µs |
| 2000 | fuzzy (distance 1) | 5 | 4 | 179 | 60.4 µs |
| 10000 | rare word | 0 | 0 | 131 | 14.9 µs |
| 10000 | no match | 0 | 0 | 13 | 5.0 µs |
| 10000 | common word, top-20 | 3226 | 20 | 2346 | 189 µs |
| 10000 | prefix (4 chars) | 217 | 20 | 817 | 182 µs |
| 10000 | infix substring (4 chars) | 70 | 19 | 798 | 66.9 µs |
| 10000 | infix substring (3 chars) | 1388 | 20 | 817 | 44.0 µs |
| 10000 | two-term AND | 4 | 4 | 628 | 40.2 µs |
| 10000 | sender facet + word | 292 | 20 | 2305 | 179 µs |
| 10000 | fuzzy (distance 1) | 25 | 13 | 542 | 149 µs |
| 50000 | rare word | 3 | 3 | 449 | 41.7 µs |
| 50000 | no match | 0 | 0 | 13 | 7.0 µs |
| 50000 | common word, top-20 | 16118 | 20 | 2430 | 351 µs |
| 50000 | prefix (4 chars) | 1072 | 20 | 817 | 324 µs |
| 50000 | infix substring (4 chars) | 342 | 20 | 817 | 295 µs |
| 50000 | infix substring (3 chars) | 6985 | 20 | 817 | 171 µs |
| 50000 | two-term AND | 21 | 10 | 1319 | 142 µs |
| 50000 | sender facet + word | 1419 | 20 | 2254 | 288 µs |
| 50000 | fuzzy (distance 1) | 120 | 20 | 817 | 360 µs |
| 200000 | rare word | 12 | 12 | 1522 | 232 µs |
| 200000 | no match | 0 | 0 | 13 | 8.7 µs |
| 200000 | common word, top-20 | 64576 | 20 | 2182 | 770 µs |
| 200000 | prefix (4 chars) | 4246 | 20 | 817 | 502 µs |
| 200000 | infix substring (4 chars) | 1356 | 20 | 817 | 958 µs |
| 200000 | infix substring (3 chars) | 27851 | 20 | 817 | 780 µs |
| 200000 | two-term AND | 86 | 16 | 1923 | 419 µs |
| 200000 | sender facet + word | 5728 | 20 | 2242 | 644 µs |
| 200000 | fuzzy (distance 1) | 463 | 20 | 817 | 459 µs |

Least-squares fit over 2608 queries: host time ≈ 7.3 µs + 17.1 ns per matched document + 13.32 µs per hit + 26.84 ns per response byte.

Peak RSS of the whole run (every section, RocksDB included): 697596 kB.

