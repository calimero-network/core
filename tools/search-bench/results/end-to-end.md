## End-to-end (live ContextManager, search-chat wasm, RocksDB), 2000 messages

Seeded both contexts (2000 messages each, 500 per execution) in 2.5 s.

Dirty log after seeding: 4 rows (one per execution; 502.0 ids, 16173 B per row on average).

Incremental, bulk: draining the 2000-message dirty backlog took 0.14 s = 69 µs per message (extract through the wasm view 47 µs, tantivy 22 µs); 4 rows, 2002 ids, 2000 docs, 1 commits.

Full build from a scan of state: 0.16 s = 82 µs per message (scan through the wasm view 50 µs, tantivy 31 µs); 2000 documents.

| one `post` execution at 2000 messages | p50 | p95 | p99 | gas |
|---|---|---|---|---|
| search off | 5.13 ms | 9.87 ms | 13.15 ms | 2.50 M |
| search on (dirty row staged in the batch) | 4.97 ms | 9.64 ms | 11.56 ms | 2.50 M |

A single post's dirty row names 3 entity ids = 205 B (key 40 + value 165), plus the 40 B counter update.

Incremental, per send: indexing the 201 single-post rows took 30.26 ms = 151 µs per post (extract 49 µs, tantivy 99 µs, 1 commits).

| query through the `search` view | total | p50 | p95 | p99 | gas |
|---|---|---|---|---|---|
| (floor: the `count` view, no search) | — | 2.30 ms | 6.26 ms | 7.28 ms | 0.09 M |
| rare word (`zebrafish`) | 2 | 2.13 ms | 5.56 ms | 7.34 ms | 0.28 M |
| common word, top-20 (`kakaka`) | 314 | 2.95 ms | 4.30 ms | 6.83 ms | 2.03 M |
| no match (`qqxqq`) | 0 | 1.89 ms | 4.08 ms | 6.12 ms | 0.08 M |
| prefix (`needl`) | 11 | 2.30 ms | 4.77 ms | 6.80 ms | 0.99 M |
| infix substring (`eedl`) | 11 | 2.14 ms | 2.67 ms | 3.90 ms | 0.99 M |
| two-term AND (`needle kakaka`) | 5 | 2.33 ms | 3.20 ms | 6.60 ms | 0.58 M |
| sender facet + word (`kakaka`) | 6 | 2.07 ms | 3.88 ms | 5.26 ms | 0.68 M |
| fuzzy (distance 1) (`neadle`) | 11 | 2.25 ms | 3.75 ms | 6.16 ms | 0.99 M |

| baseline: `scan_search` view (lowercase substring over every message) | total | p50 | p95 | gas |
|---|---|---|---|---|
| rare word (`zebrafish`) | 2 | 99.57 ms | 118.56 ms | 159.08 M |
| common word (`kakaka`) | 314 | 78.59 ms | 89.68 ms | 160.49 M |
| no match (`qqxqq`) | 0 | 80.33 ms | 115.92 ms | 159.78 M |

Freshness through the live indexer (250 ms commit interval, measured from the post returning): p50 136.39 ms, p95 252.44 ms, max 259.40 ms.

Peak RSS of the whole benchmark process (both contexts, wasm engine, RocksDB, index): 195688 kB.

## End-to-end (live ContextManager, search-chat wasm, RocksDB), 10000 messages

Seeded both contexts (10000 messages each, 500 per execution) in 12.1 s.

Dirty log after seeding: 20 rows (one per execution; 502.0 ids, 16173 B per row on average).

Incremental, bulk: draining the 10000-message dirty backlog took 0.66 s = 66 µs per message (extract through the wasm view 57 µs, tantivy 9 µs); 20 rows, 10002 ids, 10000 docs, 1 commits.

Full build from a scan of state: 0.64 s = 64 µs per message (scan through the wasm view 51 µs, tantivy 11 µs); 10000 documents.

| one `post` execution at 10000 messages | p50 | p95 | p99 | gas |
|---|---|---|---|---|
| search off | 5.30 ms | 9.72 ms | 11.95 ms | 2.53 M |
| search on (dirty row staged in the batch) | 5.40 ms | 9.63 ms | 12.91 ms | 2.53 M |

A single post's dirty row names 3 entity ids = 205 B (key 40 + value 165), plus the 40 B counter update.

Incremental, per send: indexing the 201 single-post rows took 66.16 ms = 329 µs per post (extract 53 µs, tantivy 273 µs, 1 commits).

| query through the `search` view | total | p50 | p95 | p99 | gas |
|---|---|---|---|---|---|
| (floor: the `count` view, no search) | — | 2.00 ms | 4.00 ms | 4.25 ms | 0.09 M |
| rare word (`zebrafish`) | 6 | 2.47 ms | 6.42 ms | 7.37 ms | 0.68 M |
| common word, top-20 (`kakaka`) | 1475 | 2.89 ms | 4.46 ms | 5.25 ms | 2.10 M |
| no match (`qqxqq`) | 0 | 2.04 ms | 2.42 ms | 2.51 ms | 0.08 M |
| prefix (`needl`) | 51 | 3.01 ms | 5.92 ms | 7.21 ms | 1.75 M |
| infix substring (`eedl`) | 51 | 2.73 ms | 6.47 ms | 8.98 ms | 1.75 M |
| two-term AND (`needle kakaka`) | 16 | 2.81 ms | 4.38 ms | 7.22 ms | 1.69 M |
| sender facet + word (`kakaka`) | 26 | 3.00 ms | 4.69 ms | 6.97 ms | 2.04 M |
| fuzzy (distance 1) (`neadle`) | 51 | 2.94 ms | 7.17 ms | 7.99 ms | 1.75 M |

| baseline: `scan_search` view (lowercase substring over every message) | total | p50 | p95 | gas |
|---|---|---|---|---|
| rare word (`zebrafish`) | 6 | 363.50 ms | 406.11 ms | 628.44 M |
| common word (`kakaka`) | 1475 | 343.67 ms | 384.12 ms | 633.81 M |
| no match (`qqxqq`) | 0 | 344.39 ms | 369.33 ms | 631.54 M |

Freshness through the live indexer (250 ms commit interval, measured from the post returning): p50 143.70 ms, p95 249.89 ms, max 262.38 ms.

Peak RSS of the whole benchmark process (both contexts, wasm engine, RocksDB, index): 264384 kB.

## End-to-end (live ContextManager, search-chat wasm, RocksDB), 50000 messages

test handlers::execute::search_tests::bench::search_e2e_bench has been running for over 60 seconds
Seeded both contexts (50000 messages each, 500 per execution) in 64.1 s.

Dirty log after seeding: 100 rows (one per execution; 502.0 ids, 16173 B per row on average).

Incremental, bulk: draining the 50000-message dirty backlog took 3.77 s = 75 µs per message (extract through the wasm view 68 µs, tantivy 6 µs); 100 rows, 50002 ids, 50000 docs, 1 commits.

Full build from a scan of state: 3.69 s = 74 µs per message (scan through the wasm view 67 µs, tantivy 7 µs); 50000 documents.

| one `post` execution at 50000 messages | p50 | p95 | p99 | gas |
|---|---|---|---|---|
| search off | 4.83 ms | 9.03 ms | 14.55 ms | 2.59 M |
| search on (dirty row staged in the batch) | 5.03 ms | 9.14 ms | 11.44 ms | 2.59 M |

A single post's dirty row names 3 entity ids = 205 B (key 40 + value 165), plus the 40 B counter update.

Incremental, per send: indexing the 201 single-post rows took 38.66 ms = 192 µs per post (extract 69 µs, tantivy 119 µs, 1 commits).

| query through the `search` view | total | p50 | p95 | p99 | gas |
|---|---|---|---|---|---|
| (floor: the `count` view, no search) | — | 2.29 ms | 2.64 ms | 2.95 ms | 0.09 M |
| rare word (`zebrafish`) | 26 | 4.06 ms | 6.90 ms | 10.17 ms | 2.08 M |
| common word, top-20 (`kakaka`) | 7414 | 3.31 ms | 7.35 ms | 7.71 ms | 2.27 M |
| no match (`qqxqq`) | 0 | 2.55 ms | 4.43 ms | 7.23 ms | 0.08 M |
| prefix (`needl`) | 251 | 4.11 ms | 15.13 ms | 32.07 ms | 1.76 M |
| infix substring (`eedl`) | 251 | 3.01 ms | 5.04 ms | 7.01 ms | 1.75 M |
| two-term AND (`needle kakaka`) | 39 | 3.21 ms | 4.82 ms | 6.07 ms | 2.10 M |
| sender facet + word (`kakaka`) | 123 | 3.20 ms | 6.72 ms | 7.45 ms | 2.06 M |
| fuzzy (distance 1) (`neadle`) | 251 | 2.99 ms | 4.13 ms | 8.77 ms | 1.76 M |

| baseline: `scan_search` view (lowercase substring over every message) | total | p50 | p95 | gas |
|---|---|---|---|---|
| rare word (`zebrafish`) | gas exhausted | 673.45 ms | 772.12 ms | 1000.00 M |
| common word (`kakaka`) | gas exhausted | 658.14 ms | 687.82 ms | 1000.00 M |
| no match (`qqxqq`) | gas exhausted | 671.10 ms | 694.50 ms | 1000.00 M |

Freshness through the live indexer (250 ms commit interval, measured from the post returning): p50 148.31 ms, p95 235.78 ms, max 259.32 ms.

Peak RSS of the whole benchmark process (both contexts, wasm engine, RocksDB, index): 587052 kB.

## End-to-end (live ContextManager, search-chat wasm, RocksDB), 200000 messages

test handlers::execute::search_tests::bench::search_e2e_bench has been running for over 60 seconds
Seeded both contexts (200000 messages each, 500 per execution) in 268.0 s.

Dirty log after seeding: 400 rows (one per execution; 502.0 ids, 16173 B per row on average).

Incremental, bulk: draining the 200000-message dirty backlog took 21.40 s = 107 µs per message (extract through the wasm view 98 µs, tantivy 7 µs); 400 rows, 200002 ids, 200000 docs, 1 commits.

Full build from a scan of state: 22.32 s = 112 µs per message (scan through the wasm view 102 µs, tantivy 7 µs); 200000 documents.

| one `post` execution at 200000 messages | p50 | p95 | p99 | gas |
|---|---|---|---|---|
| search off | 6.53 ms | 13.03 ms | 17.61 ms | 2.67 M |
| search on (dirty row staged in the batch) | 6.82 ms | 14.11 ms | 16.65 ms | 2.67 M |

A single post's dirty row names 3 entity ids = 205 B (key 40 + value 165), plus the 40 B counter update.

Incremental, per send: indexing the 201 single-post rows took 62.11 ms = 309 µs per post (extract 91 µs, tantivy 214 µs, 1 commits).

| query through the `search` view | total | p50 | p95 | p99 | gas |
|---|---|---|---|---|---|
| (floor: the `count` view, no search) | — | 2.38 ms | 4.75 ms | 6.25 ms | 0.09 M |
| rare word (`zebrafish`) | 101 | 3.73 ms | 6.56 ms | 7.95 ms | 2.09 M |
| common word, top-20 (`kakaka`) | 29373 | 3.94 ms | 8.09 ms | 8.68 ms | 2.82 M |
| no match (`qqxqq`) | 0 | 2.28 ms | 5.01 ms | 5.64 ms | 0.08 M |
| prefix (`needl`) | 1001 | 3.71 ms | 8.49 ms | 10.38 ms | 1.77 M |
| infix substring (`eedl`) | 1001 | 3.99 ms | 9.20 ms | 16.06 ms | 1.77 M |
| two-term AND (`needle kakaka`) | 155 | 4.59 ms | 8.62 ms | 11.83 ms | 2.12 M |
| sender facet + word (`kakaka`) | 553 | 3.90 ms | 8.26 ms | 8.71 ms | 2.08 M |
| fuzzy (distance 1) (`neadle`) | 1001 | 4.11 ms | 8.40 ms | 10.52 ms | 1.77 M |

| baseline: `scan_search` view (lowercase substring over every message) | total | p50 | p95 | gas |
|---|---|---|---|---|
| rare word (`zebrafish`) | gas exhausted | 906.67 ms | 946.17 ms | 1000.00 M |
| common word (`kakaka`) | gas exhausted | 901.51 ms | 984.98 ms | 1000.00 M |
| no match (`qqxqq`) | gas exhausted | 853.05 ms | 921.34 ms | 1000.00 M |

Freshness through the live indexer (250 ms commit interval, measured from the post returning): p50 151.08 ms, p95 264.34 ms, max 1990.81 ms.

Peak RSS of the whole benchmark process (both contexts, wasm engine, RocksDB, index): 1271764 kB.


## Inserts under the default gas budget (1000.00 M gas)

| search | gas of 1 / 10 / 100 / 200 inserts in one call | fixed per call | per insert (100 → 200) | most inserts in one call | what stops the next one |
|---|---|---|---|---|---|
| off | 2.29 M / 15.19 M / 152.73 M / 308.47 M | 0.73 M | 1.56 M | 636 (999.25 M gas) | gas exhausted |
| on | 2.29 M / 15.19 M / 152.73 M / 308.47 M | 0.73 M | 1.56 M | 636 (999.25 M gas) | gas exhausted |

| messages already in the map | one `post`, search off | one `post`, search on |
|---|---|---|
| 2000 | 2.50 M | 2.50 M |
| 10000 | 2.55 M | 2.55 M |
| 50000 | 2.58 M | 2.58 M |
| 200000 | 2.61 M | 2.61 M |
