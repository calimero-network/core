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

## Follow-up: storage tree performance

This section looks at where the tree itself spends rows and time, not the row codec. The
tree here is the Merkle walk, the child trie, the collections over them and the
delta-apply path. Row counts come from `tools/storage-cost` (deterministic) and from a
host-call tracer. Times come from an in-memory store, so they are storage-crate CPU only,
as in the table above. All builds are release builds in a target directory of their own,
run on the shared 4-core container at load 2.5 to 5. Each time is the median of five
runs, interleaved across the three builds.

### What was wrong, ranked

**1. Every write walked its ancestors twice, at about four times the rows each level
needs (fixed).** A write stores its entity's new hash in the parent's child trie, then
repeats up to the context root. At each level the walk did four things:

- looked the child up: one descent of the trie, plus a read of the child's own index row
  for metadata it already held
- replaced the child: a second descent
- read the trie root back
- counted the root's children for a log line

It never stopped early. A level whose slot already held the hash still rewrote every row
above it with the bytes they held. A map insert paid for the whole walk twice: once for
the link, and once for the value write that follows it with the same bytes. The second
walk changed nothing and still rewrote the map and the root.

One `UnorderedMap` call at 1,000 entries, rows read / written:

| call | before | after |
|---|---:|---:|
| insert a new key | 38 / 10 | 23 / 8 |
| update an existing key | 23 / 7 | 14 / 7 |

Rewriting a key with the bytes it already holds now writes one row, the entry's (its
stamp moves), and walks nowhere.

**2. Applying a peer's delta did the walk's work once per action (fixed).** `Root::sync`
defers the walks to the end of the delta, but the deferral batched nothing.
`apply_action` relinked every applied entry under its parent right after writing it. That
rewrote the entry a second time, and the parent's row and its trie spine once per action.
The deferred walks then ran one id at a time, so an ancestor shared by two dirty entities
was rewritten once per entity beneath it. A delta updating 1,000 entries of one map wrote
the map's row and its trie root 1,000 times each.

**3. The local write path reads the same rows repeatedly (not fixed; overlaps the codec
work).** After fix 1, an insert still reads the new entry's row 7 times and the map's row
4 times. The entry's reads come from `get_mut`, `write_child_index`, `save_raw_stamped`,
twice from `save_internal`, `rehashed` and `get_delta_ancestors_of`. Only about 10 of the
insert's 23 reads are distinct rows. An update reads the entry's row 6 times out of 14.

The fix is to pass the index already loaded through `save_raw_stamped`, `save_internal`
and `write_value_for`. It would cut roughly 40% of the reads of every write. It changes
nothing but the number of host calls, so gas stays equal across replicas. The row-codec
work is rewriting this same code (`interface.rs`, `index.rs`), so it is left to that
change.

**4. An insert descends the parent's trie three times (not fixed).**
`Interface::add_child_to` first looks the child up to keep any position it already holds:
a full descent plus the child's index row. It then reads the root row again for
`next_order`, and the link descends a third time. That is 7 of an insert's 23 reads at
1,000 entries. A single descent that links and reports the position already held would
save about 4 reads (17%). That descent has to settle the position before the entry's
bytes are hashed, which needs a restructured `add_child_to` (`interface.rs`).

**5. `Vector::get(i)`, `update(i)` and `remove(i)` are linear (format change, proposal
only).** A position is the rank in `(created_at, order, id)`, and the id-keyed trie cannot
answer that. So the collection loads every child and its index row: `vector_get_nth`
reads 14,013 rows at 10,000 entries.

A node-local position index cannot fix this, because a read's gas would then differ
between a replica that has the index and one that does not. A fix needs a replicated
order-statistic tree beside the trie: rows keyed by `(created_at, order, id)` with
subtree counts, maintained by every link, unlink and apply path, and carried in
snapshots. That is a new stored row kind, so a format change. Estimated effect: `get(i)`
falls from about 1.4 rows per element to about `log16(n) + 2` rows (about 6 at 10,000),
and each push writes about 3 more rows.

**6. `child_trie/get` is 2 to 2.5x slower than before #4210 (inherent; nothing to fix
here).** A bucket no longer carries each child's metadata, so `ChildTrie::get` has to read
and decode the child's own index row. That is the format working as designed: otherwise
the metadata would be stored twice and go stale. Callers that need only presence should
call `contains`.

- The ancestor walk was the hot caller, and fix 1 removed it.
- `remove_child_from_inner` still calls `get`, but that costs one read per delete and is
  not worth the churn.
- The decode CPU belongs to the codec.

**7. `merge_root_state` is 29 to 40% slower (not the tree).** The bench calls
`merge_root_state_typed`, which never touches the store or the index. It does two borsh
decodes, one `Mergeable::merge` under `with_merge_mode`, and one borsh encode. Its
regression is outside the tree and was not chased here.

**Fine as it is.**

- Trie depth and link cost grow as `log16(n / 16)`. Before the fixes, reads per insert
  went from 35.1 at 1,000 entries to 37.5 at 10,000. After, they go from 20.4 to 22.0.
- `len`, `contains` and keyed `get` cost a constant 1 to 2 rows. The guarded collections'
  counts come from the tally.
- Enumeration costs one row per bucket plus one index row per child, which an ordered
  read needs anyway.
- The remaining linear reads are linear by design and documented as such:
  `fugue_text_char_at`, `rga_get_nth`, `rich_text_to_delta`,
  `indexed_map_first_query_after_sync` and `unordered_map_filter_scan`.

### What changed

**`perf(storage): walk each ancestor once, and only while a hash moves`**

- `ChildTrie::refresh` stores a linked child's hash in one descent, and writes nothing
  when the hash is already there.
- The walk stops at the first parent whose slot already holds the hash. This is sound
  because every path that moves a full hash walks from the entity it moved.
- `write_value_for` and `add_child_with_value_to` walk only when a full hash moved.
- The child count used only in a log line is read only when that log event is enabled.

Files: `index.rs` (`recalculate_ancestor_hashes_for_now`, `write_value_for`, `rehashed`,
`add_child_with_value_to`), `child_trie.rs` (`refresh`), `admitted_count.rs`
(`before_change_at`).

**`perf(storage): apply a delta's walks as one pass over shared ancestors`**

- The deferred walks run as one pass, `Index::recalculate_ancestor_hashes_for_all`. Each
  ancestor is read once, and parents are processed deepest first.
- Each parent's dirty children go through `ChildTrie::refresh_all` in a single descent,
  which writes each trie row once however many children under it moved.
- `apply_action` relinks an entity only when its parent does not list it yet. This is a
  one-line guard in `interface.rs`.
- The deferred set is now taken off the thread before the flush. A failed `finish()` used
  to leave it behind, which deferred every later walk on that thread.

Files: `index.rs`, `child_trie.rs`, `interface.rs` (the relink guard only). Also adds the
workload `unordered_map_sync_update`.

### Evidence

Rows per call from `storage-costs.json` at the largest size, before → after both fixes:

| workload | n | rows read | rows written |
|---|---:|---|---|
| `unordered_map_insert` | 10,000 | 375,267 → 219,625 (−41%) | 108,254 → 88,252 (−18%) |
| `nested_map_insert` | 10,000 | 545,330 → 299,665 (−45%) | 138,266 → 98,260 (−29%) |
| `vector_push` | 10,000 | 365,307 → 209,665 (−43%) | 108,260 → 88,258 (−18%) |
| `indexed_map_update` | 10,000 | 26 → 16 (−38%) | 8 → 8 |
| `lww_register_set` | 10,000 | 240,039 → 190,033 (−21%) | 40,008 → 40,007 |
| `authored_map_insert` | 10,000 | 50 → 34 (−32%) | 12 → 10 (−17%) |
| `authored_vector_push` | 10,000 | 45 → 29 (−36%) | 12 → 10 (−17%) |
| `fugue_text_insert` | 10,000 | 1,317 → 763 (−42%) | 366 → 284 (−22%) |
| `rich_document_insert_block` | 2,000 | 185 → 116 (−37%) | 46 → 39 (−15%) |
| `rich_text_mark` | 10,000 | 71 → 58 (−18%) | 8 → 6 (−25%) |
| `unordered_map_sync_update` (new) | 10,000 | 310,008 → 153,987 (−50%) | 90,002 → 13,986 (−84%) |
| `rga_insert_interleaved_sync` | 2,000 | 5,599,705 → 5,531,965 (−1%) | 38,785 → 19,915 (−49%) |
| `fugue_text_insert_interleaved_sync` | 2,000 | 3,013,132 → 2,951,585 (−2%) | 35,438 → 18,694 (−47%) |

No read-only workload moved. The interleaved-sync reads barely moved because they are
dominated by the local inserts, which re-linearise the document each time. For the sync
update delta, fix 1 alone took it from 29.0 / 8.0 rows per entry to 23.0 / 6.0, and fix 2
then took it to 14.3 / 1.3.

Time per call in µs, median of five interleaved runs. Local calls run after 1,000
prefilled entries, 500 calls each. Delta rows are per action, in deltas of 100 actions.

| call | before | fix 1 | fix 1 + 2 |
|---|---:|---:|---:|
| kv set, new key | 199 | 139 | 144 |
| kv update | 155 | 123 | 122 |
| chat send (`AuthoredVector`) | 219 | 157 | 157 |
| apply a delta of 100 new keys | 136 | 97 | 67 |
| apply a delta of 100 updates | 97 | 71 | 39 |

A local call gets 21% to 30% faster, and each applied action 51% to 60% faster. That more
than recovers the per-call CPU regression measured above. Fix 2 does not touch the local
path, so its kv set median of 144 against fix 1's 139 is noise: its five runs spanned 138
to 211.

Stored bytes are unchanged. For each of the 160 storage-cost workloads, a SHA-256 over
every key and value of the resulting store (state rows, ordered index and index meta) is
the same before and after.

`tests/ancestor_walk_cost.rs` pins these costs:

- An update reads each trie row once and writes 7 rows.
- An insert writes the root's row once.
- A rewrite of the bytes already stored writes only the entry.
- A delta updating 1 or 64 entries writes no row more than once, and the receiver's root
  equals the sender's.
- Every case also checks the root against a store built directly with the final contents.
