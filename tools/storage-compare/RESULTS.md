# Storage before and after: results (2026-10-02)

This compares `02b9bd6` (master just before #4200, the first storage-efficiency PR)
with `562b36a` (master on 2026-10-02). Both builds ran on the same 4-core cloud
container, one after the other, with nothing else running.

Reproduce: see [`README.md`](README.md). The "before" build is this crate copied into
a worktree at `02b9bd6`, with `src/config.rs` returning `Options::default()`.

## State on disk

4 contexts. 25,000 kv entries (`UnorderedMap<String, LwwRegister<String>>`, the kv-store
layout) or 10,000 chat messages (`AuthoredVector<Message>`, mero-chat's message shape)
per context. "Production options" means the options that commit's node gives the `State`
column family. Before #4200 that meant no compression at all.

| per entry | before | after | change |
|---|---:|---:|---:|
| kv: rows | 3.00 | 1.17 | −61% |
| kv: logical key+value bytes | 601 B | 221 B | **−63%** |
| kv: on disk, uncompressed | 553 B | 201 B | −64% |
| kv: on disk, production options | 553 B | 130 B | **−76%** |
| chat: rows | 3.32 | 1.40 | −58% |
| chat: logical key+value bytes | 754 B | 381 B | **−49%** |
| chat: on disk, uncompressed | 706 B | 359 B | −49% |
| chat: on disk, production options | 706 B | 153 B | **−78%** |

The chat probe is a bare `AuthoredVector`. #4210 measured the whole mero-chat contract,
which has more nested collections, and got 5,658 B → 1,241 B per message for that PR
alone.

## Delta bytes per call

One commit per call, 500 calls after 1,000 prefilled. Each figure is the median of
the delta artifact the storage layer hands to the node. The node's DAG envelope
(parents, signature and so on) is not included.

| call | before | after | change |
|---|---:|---:|---:|
| kv set, new key | 708 B | 147 B | **−79%** |
| kv update, existing key | 708 B | 123 B | **−83%** |
| chat send | 927 B | 391 B | **−58%** |

## Row I/O (checked-in `storage-costs.json`, n = 1,000)

| workload | rows read | rows written |
|---|---|---|
| `unordered_map_insert` | 48,063 → 35,119 (−27%) | 18,025 → 9,906 (−45%) |
| `nested_map_insert` | 76,174 → 52,187 (−31%) | 22,067 → 12,924 (−41%) |
| `lww_register_set` | 36,044 → 24,039 (−33%) | 10,016 → 4,008 (−60%) |
| `fugue_text_insert_per_char` | 42,029 → 20,560 (−51%) | 14,041 → 5,024 (−64%) |
| `unordered_map_get` | 2 → 1 (−50%) | — |

## CPU per call: a regression

The same 500 calls, timed against an in-memory store, so no RocksDB is involved. This
is storage-crate CPU only. Each figure is the median per call, and three runs at each
commit agreed within ±2%.

| call | before | after | change |
|---|---:|---:|---:|
| kv set, new key | 159 µs | 199 µs | **+25%** |
| kv update, existing key | 133 µs | 155 µs | **+17%** |
| chat send | 164 µs | 226 µs | **+38%** |

Bisect (kv set / kv update / chat send, µs, each the mean of two runs):

| commit | kv set | kv update | chat send |
|---|---:|---:|---:|
| `02b9bd6` before | 159 | 133 | 164 |
| `a5bc66f` #4210 format change | 177 | 144 | 186 |
| `0b64c6e` #4266 one row per entity | **292** | **246** | **320** |
| `ad12c63` #4289 stamp after stored | 304 | 263 | 331 |
| `d911a6a` #4285 one row write | 237 | 200 | 257 |
| `03c2c2e` #4282 no root in chains | 216 | 180 | 237 |
| `079662a` #4335 read row once | 208 | 173 | 234 |
| `5bd3f52` #4339 update skips parent | 198 | 154 | 221 |
| `6994d78` #4340 register stamp | 196 | 155 | 223 |
| `562b36a` HEAD | 199 | 155 | 226 |

#4266 added about 65% on top of #4210. Later PRs won roughly half of that back, but the
net is still +17–38% over the start. The likely cause, not yet profiled: since #4266,
every `row::decode` computes `Sha256(data)`, to derive `own_hash` and to check the row is
canonical. When `full_hash` is not stored it often computes a second hash in
`childless_full_hash`, and it re-serializes the index with borsh. Every `row::encode`
decodes the `EntityIndex` again and hashes the data. A logical read of only `Key::Entry`
pays for all of this as well.

In the full probe this CPU cost is outweighed by I/O: the whole run (including the RocksDB
loads) takes 14 s before and 12 s after. Whether a node is faster or slower overall depends
on whether a call is bound by CPU or by I/O.

## Criterion benches

CRITERION_PLACEHOLDER
