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

The bench sources are unchanged between the two commits, so they measure the same work.
Each figure is criterion's median: `--save-baseline before` at `02b9bd6`, then
`--baseline before` at HEAD, on the same machine.

`calimero-storage`:

| bench | before | after | change |
|---|---:|---:|---:|
| `child_trie/insert/1000` | 104.8 µs | 22.2 µs | **−79%** |
| `child_trie/insert/10000` | 177.9 µs | 32.8 µs | **−82%** |
| `child_trie/children/1000` | 6.80 ms | 0.59 ms | **−91%** |
| `child_trie/children/10000` | 45.6 ms | 16.0 ms | **−65%** |
| `child_trie/get/1000` | 2.1 µs | 4.7 µs | **+130%** |
| `child_trie/get/10000` | 3.4 µs | 8.2 µs | **+145%** |
| `child_trie/root/10` | 3.9 µs | 6.6 µs | +69% |
| `child_trie/root/1000` | 6.4 µs | 5.6 µs | −12% |
| `merge_root_state/1000` | 386 µs | 541 µs | **+40%** |
| `merge_root_state/10000` | 5.10 ms | 6.60 ms | **+29%** |

The other sizes (10, 100) follow the same direction. The trie got much cheaper to insert
into and to enumerate. A point `get` now costs 2–2.5x as much, because a bucket no
longer carries the child's metadata and the child's own row has to be read (#4210,
#4266). The merge framework around one `Mergeable::merge` is 29–40% slower.

`calimero-store` (`db_ops`: raw `put` / `get_hit` / `get_miss` against the in-memory
and RocksDB stores, at 100 / 1,000 / 10,000 keys): every result is within ±11% and
moves in both directions. That is noise, which is expected, because these benches
write fixed small values and none of the format changes reach them.

## Bottom line

- **Size:** state is 63% smaller logically and 76–78% smaller on disk. Deltas are 58–83%
  smaller, and writes touch 40–60% fewer rows. That is the main win, and it is large.
- **CPU:** per call it is 17–38% *slower* than before the work started, almost all from
  #4266, and `child_trie/get` and `merge_root_state` regressed. No timing gate caught
  this: the `Benchmarks` workflow only compares on PRs labelled `run-benchmarks`.
  Worth a follow-up. Profile `row::decode`/`row::encode` first, and in particular the
  `Sha256(data)` work that runs on every decode.

## Follow-up: row codec CPU

`f3ab940` (HEAD) against `c9bcfe4` (this change), both built from clean in one worktree's
own target directory. The container was shared with three other agents, so wall times
are noisy; the instruction counts are exact.

### What the profile showed

Callgrind over the three call workloads at HEAD (release + debuginfo, only the call
workloads collected): **SHA-256 was 54.6% of all instructions**. Inclusive, `row::load`
took 34%, `EntityIndex::try_from_slice` 13%, `to_vec::<EntityIndex>` 11% and
`row::encode` 11%. A kv write read about 117 index rows (350,155 `get_index` calls for
the kv workloads' 3,000 writes), and each one cost four SHA-256s:

- `Sha256(data)` in `SlimIndex::finish`
- `H(own_hash)` there too, for the full hash
- `H(own_hash)` again when `decode` re-encoded the index with borsh
- `H(own_hash)` once more when the caller parsed those bytes back

Each index write parsed the bytes it was handed (one hash), hashed the data and
serialized slim (one more hash), after the caller had serialized it (one more). A
`Key::Entry` read paid three hashes for an index it threw away.

The hypothesis holds, with one correction: the second hash is not "often" but on every
decode, and the borsh round trip adds two more on top.

### What changed (`crates/storage/src/row.rs`, `store.rs`, `env.rs`, `index.rs`, `child_trie.rs`, `interface.rs`)

- An index crosses the adaptor decoded. `StorageAdaptor` gains `storage_read_index` and
  `storage_write_index`, and `storage_read_entity` / `storage_write_entity` take and
  return an `EntityIndex`. The defaults do exactly what the old byte calls did, and
  `MainStorage` / `PrivateStorage` override them to skip the borsh round trip.
- A read-modify-write keeps `Sha256(data)` from its decode and reuses it in its encode.
- A `Key::Entry` read runs every check of `decode` except the two that need a hash. It
  returns no index, so nothing it returns relies on them. A row that fails only those
  checks now yields its data, as `calimero_prelude::row::data` already did.
- `decode` is unchanged and still canonical, and every index handed to a caller still
  passes all of it. Host calls are unchanged: no read is cached or skipped.

### Bytes: unchanged

- `cargo run -p storage-cost --bin storage-cost --release > tools/storage-cost/storage-costs.json`
  leaves the file byte-identical, and `./scripts/check-storage-cost.sh` reports all 156
  rows matching.
- In every `storage-compare` run of both builds, rows, logical bytes, uncompressed bytes
  and delta bytes are identical. The "production options" column moves by ±0.3 B between
  runs of the *same* binary (130.3–130.6, 153.3–153.9). That is RocksDB compression run
  to run, and both builds show the same spread.

### µs per call (`storage-compare`, median of 500 calls)

Five rounds, interleaved head/new, 1-minute load ≈ 2.0 (two of the four cores busy
elsewhere). Each cell is the median of the five runs, with the range in brackets.

| call | HEAD `f3ab940` | this change | change |
|---|---:|---:|---:|
| kv set, new key | 201.0 µs [199.0–254.8] | 145.7 µs [143.8–149.2] | **−28%** |
| kv update, existing key | 157.2 µs [156.4–160.6] | 113.9 µs [112.5–116.3] | **−28%** |
| chat send | 226.9 µs [224.0–243.1] | 165.4 µs [160.5–295.2] | **−27%** |

Two of the five chat runs of the new build read ~293 µs. The other three, and all
instruction counts, agree with the rest. An earlier pass of four rounds at load 3.6–5.0
was too noisy to use (head kv set spanned 201–300 µs).

HEAD here matches the "after" row above (199 / 155 / 226). Against `02b9bd6`
(159 / 133 / 164), kv set is now −8%, kv update −14% and chat send +1%. That comparison
is across sessions, so treat it as approximate.

### Instructions (callgrind, exact)

Each workload counts its 1,000-entry prefill plus the 500 timed calls.

| workload | HEAD | this change | change |
|---|---:|---:|---:|
| kv set | 2,151.8 M | 1,656.1 M | −23.0% |
| kv update | 1,948.7 M | 1,504.6 M | −22.8% |
| chat send | 2,533.0 M | 1,943.1 M | −23.3% |
| of which SHA-256 compression | 3,618.9 M | 2,628.9 M | −27.4% |

### Criterion (`cargo bench -p calimero-storage`, `--save-baseline head` then `--baseline head`)

These are the medians from the quieter pass (load ≈ 2).

| bench | HEAD | this change | change |
|---|---:|---:|---:|
| `child_trie/get/1000` | 5.60 µs | 5.33 µs | −5% (p = 0.10) |
| `child_trie/get/10000` | 9.37 µs | 8.64 µs | −8% |
| `child_trie/root/1000` | 6.41 µs | 6.40 µs | 0% |
| `child_trie/children/1000` | 641 µs | 612 µs | −5% |
| `child_trie/children/10000` | 18.6 ms | 20.1 ms | +8% |
| `child_trie/insert/1000` | 24.3 µs | 24.4 µs | 0% |
| `merge_root_state/100` | 63.1 µs | 62.0 µs | −2% |
| `merge_root_state/1000` | 612 µs | 599 µs | −2% |
| `merge_root_state/10000` | 7.10 ms | 7.38 ms | +4% (p = 0.06) |

These benches do not reach the codec, so they cannot show this change:

- The `merge_root_state` binary is byte-identical between the two builds: it is pure
  borsh plus `with_merge_mode`, with no row read. Its spread is this machine's noise
  floor. A noisier first pass put the same binary at +21%.
- `child_trie` links bare ids with no index rows, so `hydrate` reads a missing row and
  decodes nothing.

Every `child_trie` move is within ±8%, which is that same noise. The 2–2.5x
`child_trie/get` regression since `02b9bd6` is the extra row read itself (#4210, #4266),
and `merge_root_state`'s is not in this crate's row path at all. This change leaves both
alone.

### What is left

SHA-256 is still 51.5% of the instructions. Most of it is the one `Sha256(data)` each
index read now costs: a derived `own_hash` is that hash, and an explicit one must be
checked against it. Another ~10% (of the kv calls) is `child_trie::addr`. The lever now is how many rows
a call reads, about 117 index reads per map write. Cutting that changes row counts, and
the cost gate pins those, so it is a separate change.
