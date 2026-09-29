## End-to-end (live ContextManager, search-chat wasm, RocksDB), 2000 messages
Seeded both contexts (2000 messages each, 500 per execution) in 2.5 s.
Dirty log after seeding: 4 rows (one per execution; 502.0 ids, 16108 B per row on average).
Incremental, bulk: draining the 2000-message dirty backlog took 0.24 s = 119 µs per message (extract through the wasm view 76 µs, tantivy 43 µs); 4 rows, 2002 ids, 2000 docs, 1 commits.
Full build from a scan of state: 0.25 s = 124 µs per message (scan through the wasm view 75 µs, tantivy 48 µs); 2000 documents.
| one `post` execution at 2000 messages | p50 | p95 | p99 | gas |
|---|---|---|---|---|
| search off | 5.62 ms | 8.16 ms | 9.15 ms | 2.50 M |
| search on (dirty row staged in the batch) | 5.80 ms | 10.25 ms | 11.81 ms | 2.50 M |
A single post's dirty row names 3 entity ids = 140 B (key 40 + value 100).
Incremental, per send: indexing the 201 single-post rows took 50.58 ms = 252 µs per post (extract 82 µs, tantivy 167 µs, 1 commits).
| query through the `search` view | total | p50 | p95 | p99 | gas |
|---|---|---|---|---|---|
| (floor: the `count` view, no search) | — | 2.21 ms | 3.43 ms | 13.10 ms | 0.09 M |
| rare word (`zebrafish`) | 2 | 2.77 ms | 7.08 ms | 8.81 ms | 0.20 M |
| common word, top-20 (`kakaka`) | 314 | 3.99 ms | 8.08 ms | 8.61 ms | 1.32 M |
| no match (`qqxqq`) | 0 | 2.29 ms | 4.84 ms | 6.36 ms | 0.07 M |
| prefix (`needl`) | 11 | 3.26 ms | 5.02 ms | 5.41 ms | 0.71 M |
| infix substring (`eedl`) | 11 | 2.86 ms | 4.59 ms | 5.26 ms | 0.71 M |
| two-term AND (`needle kakaka`) | 5 | 2.76 ms | 4.77 ms | 6.04 ms | 0.39 M |
| sender facet + word (`kakaka`) | 6 | 3.00 ms | 4.27 ms | 7.82 ms | 0.46 M |
| fuzzy (distance 1) (`neadle`) | 11 | 2.83 ms | 3.47 ms | 3.54 ms | 0.71 M |
| baseline: `scan_search` view (lowercase substring over every message) | total | p50 | p95 | gas |
|---|---|---|---|---|
| rare word (`zebrafish`) | 2 | 105.16 ms | 114.74 ms | 159.08 M |
| common word (`kakaka`) | 314 | 97.27 ms | 109.97 ms | 160.49 M |
| no match (`qqxqq`) | 0 | 103.11 ms | 111.95 ms | 159.78 M |
Freshness through the live indexer (250 ms commit interval, measured from the post returning): p50 162.34 ms, p95 255.02 ms, max 274.51 ms.
Peak RSS of the whole benchmark process (both contexts, wasm engine, RocksDB, index): 181864 kB.

## End-to-end (live ContextManager, search-chat wasm, RocksDB), 10000 messages
Seeded both contexts (10000 messages each, 500 per execution) in 13.8 s.
Dirty log after seeding: 20 rows (one per execution; 502.0 ids, 16108 B per row on average).
Incremental, bulk: draining the 10000-message dirty backlog took 0.95 s = 95 µs per message (extract through the wasm view 79 µs, tantivy 15 µs); 20 rows, 10002 ids, 10000 docs, 1 commits.
Full build from a scan of state: 0.96 s = 96 µs per message (scan through the wasm view 73 µs, tantivy 22 µs); 10000 documents.
| one `post` execution at 10000 messages | p50 | p95 | p99 | gas |
|---|---|---|---|---|
| search off | 5.51 ms | 9.78 ms | 15.29 ms | 2.53 M |
| search on (dirty row staged in the batch) | 5.71 ms | 8.44 ms | 11.92 ms | 2.53 M |
A single post's dirty row names 3 entity ids = 140 B (key 40 + value 100).
Incremental, per send: indexing the 201 single-post rows took 42.31 ms = 211 µs per post (extract 72 µs, tantivy 132 µs, 1 commits).
| query through the `search` view | total | p50 | p95 | p99 | gas |
|---|---|---|---|---|---|
| (floor: the `count` view, no search) | — | 1.77 ms | 2.29 ms | 3.83 ms | 0.09 M |
| rare word (`zebrafish`) | 6 | 2.44 ms | 2.97 ms | 5.18 ms | 0.46 M |
| common word, top-20 (`kakaka`) | 1475 | 3.57 ms | 6.95 ms | 9.10 ms | 1.36 M |
| no match (`qqxqq`) | 0 | 1.81 ms | 2.16 ms | 5.44 ms | 0.07 M |
| prefix (`needl`) | 51 | 3.09 ms | 5.09 ms | 7.65 ms | 1.25 M |
| infix substring (`eedl`) | 51 | 3.54 ms | 6.64 ms | 14.90 ms | 1.24 M |
| two-term AND (`needle kakaka`) | 16 | 3.02 ms | 7.77 ms | 9.20 ms | 1.11 M |
| sender facet + word (`kakaka`) | 26 | 3.44 ms | 6.66 ms | 6.86 ms | 1.35 M |
| fuzzy (distance 1) (`neadle`) | 51 | 3.23 ms | 5.87 ms | 7.11 ms | 1.25 M |
| baseline: `scan_search` view (lowercase substring over every message) | total | p50 | p95 | gas |
|---|---|---|---|---|
| rare word (`zebrafish`) | 6 | 361.16 ms | 380.70 ms | 628.44 M |
| common word (`kakaka`) | 1475 | 375.44 ms | 396.94 ms | 633.81 M |
| no match (`qqxqq`) | 0 | 359.94 ms | 378.63 ms | 631.54 M |
Freshness through the live indexer (250 ms commit interval, measured from the post returning): p50 151.10 ms, p95 264.60 ms, max 268.56 ms.
Peak RSS of the whole benchmark process (both contexts, wasm engine, RocksDB, index): 237332 kB.

## End-to-end (live ContextManager, search-chat wasm, RocksDB), 50000 messages
Seeded both contexts (50000 messages each, 500 per execution) in 72.5 s.
Dirty log after seeding: 100 rows (one per execution; 502.0 ids, 16108 B per row on average).
Incremental, bulk: draining the 50000-message dirty backlog took 5.65 s = 113 µs per message (extract through the wasm view 101 µs, tantivy 11 µs); 100 rows, 50002 ids, 50000 docs, 1 commits.
Full build from a scan of state: 4.81 s = 96 µs per message (scan through the wasm view 84 µs, tantivy 11 µs); 50000 documents.
| one `post` execution at 50000 messages | p50 | p95 | p99 | gas |
|---|---|---|---|---|
| search off | 7.24 ms | 11.64 ms | 15.97 ms | 2.59 M |
| search on (dirty row staged in the batch) | 6.94 ms | 9.92 ms | 13.36 ms | 2.59 M |
A single post's dirty row names 3 entity ids = 140 B (key 40 + value 100).
Incremental, per send: indexing the 201 single-post rows took 59.51 ms = 296 µs per post (extract 121 µs, tantivy 171 µs, 1 commits).
| query through the `search` view | total | p50 | p95 | p99 | gas |
|---|---|---|---|---|---|
| (floor: the `count` view, no search) | — | 2.60 ms | 5.54 ms | 7.18 ms | 0.09 M |
| rare word (`zebrafish`) | 26 | 4.73 ms | 12.69 ms | 34.56 ms | 1.37 M |
| common word, top-20 (`kakaka`) | 7414 | 4.90 ms | 10.85 ms | 23.62 ms | 1.37 M |
| no match (`qqxqq`) | 0 | 2.41 ms | 5.78 ms | 7.94 ms | 0.07 M |
| prefix (`needl`) | 251 | 4.80 ms | 10.25 ms | 18.73 ms | 1.25 M |
| infix substring (`eedl`) | 251 | 4.29 ms | 7.56 ms | 10.38 ms | 1.24 M |
| two-term AND (`needle kakaka`) | 39 | 4.40 ms | 6.62 ms | 7.54 ms | 1.37 M |
| sender facet + word (`kakaka`) | 123 | 5.19 ms | 9.14 ms | 10.63 ms | 1.36 M |
| fuzzy (distance 1) (`neadle`) | 251 | 4.62 ms | 6.38 ms | 8.64 ms | 1.25 M |
| baseline: `scan_search` view (lowercase substring over every message) | total | p50 | p95 | gas |
|---|---|---|---|---|
| rare word (`zebrafish`) | gas exhausted | 836.20 ms | 969.45 ms | 1000.00 M |
| common word (`kakaka`) | gas exhausted | 800.10 ms | 852.74 ms | 1000.00 M |
| no match (`qqxqq`) | gas exhausted | 809.93 ms | 876.05 ms | 1000.00 M |
Freshness through the live indexer (250 ms commit interval, measured from the post returning): p50 158.96 ms, p95 262.21 ms, max 272.95 ms.
Peak RSS of the whole benchmark process (both contexts, wasm engine, RocksDB, index): 569884 kB.

## End-to-end (live ContextManager, search-chat wasm, RocksDB), 200000 messages
Seeded both contexts (200000 messages each, 500 per execution) in 345.9 s.
Dirty log after seeding: 400 rows (one per execution; 502.0 ids, 16108 B per row on average).
Incremental, bulk: draining the 200000-message dirty backlog took 30.24 s = 151 µs per message (extract through the wasm view 139 µs, tantivy 11 µs); 400 rows, 200002 ids, 200000 docs, 1 commits.
Full build from a scan of state: 25.62 s = 128 µs per message (scan through the wasm view 116 µs, tantivy 10 µs); 200000 documents.
| one `post` execution at 200000 messages | p50 | p95 | p99 | gas |
|---|---|---|---|---|
| search off | 8.47 ms | 13.90 ms | 17.46 ms | 2.67 M |
| search on (dirty row staged in the batch) | 9.03 ms | 12.64 ms | 17.16 ms | 2.67 M |
A single post's dirty row names 3 entity ids = 140 B (key 40 + value 100).
Incremental, per send: indexing the 201 single-post rows took 82.08 ms = 408 µs per post (extract 84 µs, tantivy 319 µs, 1 commits).
| query through the `search` view | total | p50 | p95 | p99 | gas |
|---|---|---|---|---|---|
| (floor: the `count` view, no search) | — | 3.41 ms | 5.95 ms | 23.43 ms | 0.09 M |
| rare word (`zebrafish`) | 101 | 6.03 ms | 11.16 ms | 13.01 ms | 1.37 M |
| common word, top-20 (`kakaka`) | 29373 | 7.18 ms | 15.18 ms | 15.83 ms | 1.37 M |
| no match (`qqxqq`) | 0 | 3.24 ms | 7.91 ms | 17.09 ms | 0.07 M |
| prefix (`needl`) | 1001 | 6.04 ms | 10.37 ms | 18.55 ms | 1.25 M |
| infix substring (`eedl`) | 1001 | 6.12 ms | 14.41 ms | 28.86 ms | 1.24 M |
| two-term AND (`needle kakaka`) | 155 | 6.24 ms | 10.15 ms | 13.52 ms | 1.38 M |
| sender facet + word (`kakaka`) | 553 | 6.27 ms | 9.61 ms | 13.11 ms | 1.36 M |
| fuzzy (distance 1) (`neadle`) | 1001 | 5.37 ms | 6.22 ms | 8.83 ms | 1.25 M |
| baseline: `scan_search` view (lowercase substring over every message) | total | p50 | p95 | gas |
|---|---|---|---|---|
| rare word (`zebrafish`) | gas exhausted | 1423.18 ms | 1514.48 ms | 1000.00 M |
| common word (`kakaka`) | gas exhausted | 1418.64 ms | 1565.83 ms | 1000.00 M |
| no match (`qqxqq`) | gas exhausted | 1359.53 ms | 1492.98 ms | 1000.00 M |
Freshness through the live indexer (250 ms commit interval, measured from the post returning): p50 157.79 ms, p95 258.68 ms, max 268.76 ms.
Peak RSS of the whole benchmark process (both contexts, wasm engine, RocksDB, index): 949908 kB.

