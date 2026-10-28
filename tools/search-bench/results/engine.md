## Engine benchmark (tools/search-bench)

Release build, one thread per query unless stated, on this machine (4 cores). Per-message times are wall-clock time of the single indexing thread (tantivy's merge thread runs beside it).

### 2000 chat messages

Corpus: 2000 messages, 11.0 words and 66 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | index bytes | per message | index / raw text |
|---|---|---|---|---|---|---|
| words | Ram | 0.04 s | 18.8 µs | 0.27 MiB | 143 B | 2.2× |
| words | Mmap | 0.03 s | 17.2 µs | 0.27 MiB | 143 B | 2.2× |
| words + trigrams | Ram | 0.04 s | 21.3 µs | 0.59 MiB | 309 B | 4.6× |
| words + trigrams | Mmap | 0.05 s | 26.1 µs | 0.59 MiB | 309 B | 4.6× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RamDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 0 | 2.8 µs | 21.0 µs | 24.9 µs | 2.9 µs | 20.7 µs | 22.2 µs |
| no match | 0 | 2.2 µs | 2.4 µs | 5.3 µs | 2.2 µs | 2.4 µs | 4.3 µs |
| common word, top-20 | 631 | 74.5 µs | 94.3 µs | 106 µs | 73.9 µs | 92.2 µs | 101 µs |
| prefix (4 chars) | 41 | 104 µs | 125 µs | 133 µs | 104 µs | 125 µs | 135 µs |
| infix substring (4 chars) | 13 | 14.4 µs | 21.4 µs | 27.4 µs | 14.0 µs | 20.8 µs | 30.5 µs |
| infix substring (3 chars) | 281 | 15.8 µs | 21.8 µs | 33.3 µs | 15.1 µs | 20.9 µs | 30.8 µs |
| two-term AND | 1 | 6.7 µs | 18.1 µs | 19.5 µs | 6.7 µs | 17.9 µs | 20.2 µs |
| sender facet + word | 57 | 71.1 µs | 86.3 µs | 98.2 µs | 71.0 µs | 87.1 µs | 100 µs |
| fuzzy (distance 1) | 5 | 32.7 µs | 59.3 µs | 77.2 µs | 32.6 µs | 61.3 µs | 76.6 µs |

Single message + its own commit at 2000 docs (RamDirectory, what a view pays to catch up on one write): p50 3238 µs, p99 22.8 ms.

Native linear scan over 2000 in-memory strings (lowercase + contains + sort; a LOWER bound for an app scanning its own state — no WASM, no storage reads): rare p50 93.4 µs / p99 143 µs, common p50 108 µs / p99 131 µs, infix p50 92.6 µs / p99 115 µs.


### 10000 chat messages

Corpus: 10000 messages, 11.1 words and 67 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | index bytes | per message | index / raw text |
|---|---|---|---|---|---|---|
| words | Ram | 0.15 s | 14.8 µs | 1.17 MiB | 122 B | 1.8× |
| words | Mmap | 0.20 s | 19.8 µs | 1.17 MiB | 122 B | 1.8× |
| words + trigrams | Ram | 0.24 s | 24.5 µs | 2.45 MiB | 256 B | 3.8× |
| words + trigrams | Mmap | 0.32 s | 32.2 µs | 2.45 MiB | 256 B | 3.8× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RamDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 1 | 3.7 µs | 21.0 µs | 22.2 µs | 3.6 µs | 21.0 µs | 23.9 µs |
| no match | 0 | 2.4 µs | 2.5 µs | 2.6 µs | 2.4 µs | 2.5 µs | 2.6 µs |
| common word, top-20 | 3186 | 91.6 µs | 112 µs | 124 µs | 92.2 µs | 112 µs | 126 µs |
| prefix (4 chars) | 209 | 161 µs | 187 µs | 205 µs | 170 µs | 231 µs | 278 µs |
| infix substring (4 chars) | 68 | 39.0 µs | 62.7 µs | 75.4 µs | 37.3 µs | 61.7 µs | 79.0 µs |
| infix substring (3 chars) | 1395 | 26.0 µs | 48.5 µs | 51.4 µs | 25.2 µs | 47.6 µs | 52.3 µs |
| two-term AND | 4 | 14.4 µs | 82.4 µs | 84.8 µs | 14.5 µs | 65.1 µs | 84.1 µs |
| sender facet + word | 278 | 89.3 µs | 119 µs | 128 µs | 90.6 µs | 117 µs | 140 µs |
| fuzzy (distance 1) | 22 | 77.0 µs | 97.7 µs | 111 µs | 78.8 µs | 101 µs | 118 µs |

Single message + its own commit at 10000 docs (RamDirectory, what a view pays to catch up on one write): p50 3366 µs, p99 6298 µs.

Native linear scan over 10000 in-memory strings (lowercase + contains + sort; a LOWER bound for an app scanning its own state — no WASM, no storage reads): rare p50 586 µs / p99 805 µs, common p50 697 µs / p99 764 µs, infix p50 567 µs / p99 706 µs.


### 50000 chat messages

Corpus: 50000 messages, 11.1 words and 67 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | index bytes | per message | index / raw text |
|---|---|---|---|---|---|---|
| words | Ram | 0.74 s | 14.8 µs | 5.43 MiB | 113 B | 1.7× |
| words | Mmap | 1.75 s | 34.9 µs | 5.43 MiB | 113 B | 1.7× |
| words + trigrams | Ram | 1.29 s | 25.8 µs | 11.31 MiB | 237 B | 3.5× |
| words + trigrams | Mmap | 2.00 s | 40.0 µs | 11.32 MiB | 237 B | 3.5× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RamDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 3 | 22.3 µs | 39.0 µs | 44.0 µs | 18.8 µs | 33.5 µs | 48.1 µs |
| no match | 0 | 3.7 µs | 3.8 µs | 4.6 µs | 3.8 µs | 3.9 µs | 4.2 µs |
| common word, top-20 | 15951 | 171 µs | 229 µs | 246 µs | 172 µs | 229 µs | 251 µs |
| prefix (4 chars) | 1058 | 274 µs | 314 µs | 361 µs | 282 µs | 367 µs | 409 µs |
| infix substring (4 chars) | 339 | 154 µs | 276 µs | 295 µs | 148 µs | 259 µs | 279 µs |
| infix substring (3 chars) | 6926 | 96.6 µs | 182 µs | 191 µs | 96.2 µs | 183 µs | 208 µs |
| two-term AND | 23 | 58.3 µs | 106 µs | 124 µs | 58.4 µs | 107 µs | 122 µs |
| sender facet + word | 1403 | 151 µs | 288 µs | 298 µs | 152 µs | 289 µs | 308 µs |
| fuzzy (distance 1) | 115 | 193 µs | 235 µs | 273 µs | 207 µs | 260 µs | 295 µs |

Single message + its own commit at 50000 docs (RamDirectory, what a view pays to catch up on one write): p50 3555 µs, p99 105.1 ms.

Native linear scan over 50000 in-memory strings (lowercase + contains + sort; a LOWER bound for an app scanning its own state — no WASM, no storage reads): rare p50 2918 µs / p99 3246 µs, common p50 3609 µs / p99 3717 µs, infix p50 2907 µs / p99 3195 µs.


### 200000 chat messages

Corpus: 200000 messages, 11.1 words and 67 bytes of text per message on average.

| schema | directory | build (1k-doc commits) | per message | index bytes | per message | index / raw text |
|---|---|---|---|---|---|---|
| words | Ram | 3.77 s | 18.8 µs | 20.23 MiB | 106 B | 1.6× |
| words | Mmap | 4.39 s | 21.9 µs | 20.23 MiB | 106 B | 1.6× |
| words + trigrams | Ram | 5.72 s | 28.6 µs | 42.40 MiB | 222 B | 3.3× |
| words + trigrams | Mmap | 7.44 s | 37.2 µs | 41.65 MiB | 218 B | 3.3× |

Correctness: 124/124 sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.

Query latency, words + trigrams index, warm, 400 timed queries per row (single thread):

| query | avg matches | RamDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |
|---|---|---|---|---|---|---|---|
| rare word | 11 | 101 µs | 172 µs | 203 µs | 94.4 µs | 169 µs | 203 µs |
| no match | 0 | 4.4 µs | 4.7 µs | 7.7 µs | 2.5 µs | 2.6 µs | 2.7 µs |
| common word, top-20 | 64088 | 406 µs | 577 µs | 656 µs | 361 µs | 531 µs | 554 µs |
| prefix (4 chars) | 4239 | 382 µs | 432 µs | 533 µs | 266 µs | 301 µs | 329 µs |
| infix substring (4 chars) | 1366 | 495 µs | 915 µs | 947 µs | 488 µs | 892 µs | 953 µs |
| infix substring (3 chars) | 27812 | 261 µs | 629 µs | 824 µs | 234 µs | 526 µs | 615 µs |
| two-term AND | 89 | 201 µs | 277 µs | 356 µs | 182 µs | 250 µs | 295 µs |
| sender facet + word | 5685 | 366 µs | 855 µs | 911 µs | 338 µs | 807 µs | 856 µs |
| fuzzy (distance 1) | 466 | 262 µs | 335 µs | 371 µs | 193 µs | 252 µs | 287 µs |

Single message + its own commit at 200000 docs (RamDirectory, what a view pays to catch up on one write): p50 3842 µs, p99 104.0 ms.

Native linear scan over 200000 in-memory strings (lowercase + contains + sort; a LOWER bound for an app scanning its own state — no WASM, no storage reads): rare p50 11.9 ms / p99 18.7 ms, common p50 15.1 ms / p99 30.7 ms, infix p50 13.5 ms / p99 18.0 ms.


### Memory (Rust heap, counting allocator; the RamDirectory's bytes included)

| state | heap |
|---|---|
| one empty index open, reader only | 2.38 MiB |
| + writer open (1 thread, 15 MB arena budget) | 2.39 MiB |
| peak while indexing 10k docs in 1k-doc commits | 28.46 MiB |
| writer closed (the directory holds 2.44 MiB) | 3.45 MiB |
| after every query kind ran | 3.58 MiB |
| 10 apps × 10k docs open for query (per app) | 30.02 MiB (3.00 MiB each) |

The node keeps every index in memory (rebuilt from state after a restart), so the per-app figure is the resident cost of one searchable app of this size.


### Across apps: 10 indexes × 10k messages

Built in 2.2 s.

| query | sequential p50 | p99 | parallel (10 threads) + merge p50 | p99 |
|---|---|---|---|---|
| rare word | 143 µs | 468 µs | 764 µs | 4052 µs |
| no match | 61.9 µs | 186 µs | 611 µs | 2039 µs |
| common word, top-20 | 1449 µs | 2382 µs | 2040 µs | 4421 µs |
| prefix (4 chars) | 2347 µs | 2716 µs | 2662 µs | 3188 µs |
| infix substring (4 chars) | 715 µs | 1315 µs | 920 µs | 1445 µs |
| two-term AND | 441 µs | 1467 µs | 1908 µs | 3967 µs |
| sender facet + word | 1334 µs | 2156 µs | 2464 µs | 6729 µs |
| fuzzy (distance 1) | 1342 µs | 2184 µs | 1401 µs | 4503 µs |

A view searches only its own app's index; this is what a client merging several apps' answers would pay. The merge sorts by raw BM25, which is only roughly comparable across indexes (each has its own IDF). Threads are spawned per query; a pool would be cheaper.


### The host time of a query, for search gas

| messages | query | matched | hits | response bytes | host p50 |
|---|---|---|---|---|---|
| 2000 | rare word | 0 | 0 | 55 | 2.8 µs |
| 2000 | no match | 0 | 0 | 13 | 2.1 µs |
| 2000 | common word, top-20 | 643 | 20 | 2445 | 81.2 µs |
| 2000 | prefix (4 chars) | 44 | 19 | 2381 | 107 µs |
| 2000 | infix substring (4 chars) | 14 | 12 | 503 | 15.2 µs |
| 2000 | infix substring (3 chars) | 273 | 20 | 817 | 16.5 µs |
| 2000 | two-term AND | 1 | 1 | 175 | 11.7 µs |
| 2000 | sender facet + word | 60 | 20 | 2424 | 75.9 µs |
| 2000 | fuzzy (distance 1) | 5 | 4 | 179 | 32.9 µs |
| 10000 | rare word | 0 | 0 | 131 | 8.0 µs |
| 10000 | no match | 0 | 0 | 13 | 2.4 µs |
| 10000 | common word, top-20 | 3226 | 20 | 2348 | 105 µs |
| 10000 | prefix (4 chars) | 217 | 20 | 2474 | 160 µs |
| 10000 | infix substring (4 chars) | 70 | 19 | 798 | 37.1 µs |
| 10000 | infix substring (3 chars) | 1388 | 20 | 817 | 25.9 µs |
| 10000 | two-term AND | 4 | 4 | 628 | 19.5 µs |
| 10000 | sender facet + word | 292 | 20 | 2306 | 91.4 µs |
| 10000 | fuzzy (distance 1) | 25 | 13 | 542 | 75.8 µs |
| 50000 | rare word | 3 | 3 | 449 | 25.0 µs |
| 50000 | no match | 0 | 0 | 13 | 4.4 µs |
| 50000 | common word, top-20 | 16118 | 20 | 2430 | 202 µs |
| 50000 | prefix (4 chars) | 1072 | 20 | 2478 | 288 µs |
| 50000 | infix substring (4 chars) | 342 | 20 | 817 | 173 µs |
| 50000 | infix substring (3 chars) | 6985 | 20 | 817 | 101 µs |
| 50000 | two-term AND | 21 | 10 | 1319 | 70.5 µs |
| 50000 | sender facet + word | 1419 | 20 | 2254 | 149 µs |
| 50000 | fuzzy (distance 1) | 120 | 20 | 817 | 175 µs |
| 200000 | rare word | 12 | 12 | 1522 | 124 µs |
| 200000 | no match | 0 | 0 | 13 | 2.9 µs |
| 200000 | common word, top-20 | 64576 | 20 | 2186 | 405 µs |
| 200000 | prefix (4 chars) | 4246 | 20 | 2366 | 287 µs |
| 200000 | infix substring (4 chars) | 1356 | 20 | 817 | 509 µs |
| 200000 | infix substring (3 chars) | 27851 | 20 | 817 | 236 µs |
| 200000 | two-term AND | 86 | 16 | 1924 | 200 µs |
| 200000 | sender facet + word | 5728 | 20 | 2242 | 398 µs |
| 200000 | fuzzy (distance 1) | 463 | 20 | 817 | 186 µs |

Least-squares fit over 2608 queries: host time ≈ 0.1 µs + 5.8 ns per matched document + 5.72 µs per hit + 33.27 ns per response byte.

### Docs: 10k documents × 50 blocks (one search document per block)

500000 blocks, 106.81 MiB of text; built (words + trigrams, RamDirectory, 5k-block commits) in 35.6 s = 71.1 µs per block; 290.56 MiB in memory (609 B per block).

| query | avg matches | p50 | p99 |
|---|---|---|---|
| rare word | 91 | 306 µs | 406 µs |
| no match | 0 | 2.8 µs | 3.0 µs |
| common word, top-20 | 352415 | 1898 µs | 2417 µs |
| prefix (4 chars) | 33884 | 582 µs | 668 µs |
| infix substring (4 chars) | 10817 | 4056 µs | 8973 µs |
| two-term AND | 860 | 451 µs | 613 µs |
| fuzzy (distance 1) | 3738 | 309 µs | 456 µs |

Peak RSS of the whole run (every section): 858708 kB.

