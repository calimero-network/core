# state-disk-cost

Measures what context state costs **on disk**, after RocksDB adds keys, block
framing, index and filter blocks, and takes away prefix delta encoding and
compression. The in-memory `storage-cost` probe counts rows and value bytes;
it cannot see any of that.

```bash
cargo run -p state-disk-cost --release
cargo run -p state-disk-cost --release -- --contexts 4 --kv 25000 --chat 10000 --only block
```

The tool generates real rows with `calimero-storage`:

- **kv**: an `UnorderedMap<String, LwwRegister<String>>`, which is the layout of `apps/kv-store`.
- **chat**: an `AuthoredVector` of messages (sender, text, timestamp, flags), shaped like mero-chat's.

It writes them under the node's `State` keys (`context_id ‖ tag ‖ id`, 65
bytes), several contexts interleaved, in batches of 32 rows in random key order. It then
flushes, forces a full compaction to the bottommost level, and reads
`rocksdb.total-sst-files-size`, `rocksdb.estimate-live-data-size` and the
aggregated table properties. Each variant starts from
`calimero_store_rocksdb::{table_options, column_options}`, so the `node` row is
exactly what a node opens `State` with.

Timings are for one run on a shared machine, so read them for ratios.
- **load**: the writes plus the flush.
- **compact**: the forced bottommost compaction.
- **gets**: 20,000 random point reads after a reopen with a 4MB block cache, much smaller than the data. They pay for block reads and decompression.

## Results (4 contexts, 100,000 kv entries / 40,000 messages)


## kv — 4 contexts, 100000 entries, 117474 rows, 1.2 rows/entry, logical 237 B/entry (keys 76 + values 161)

| variant | flushed B/entry | compacted B/entry | vs node | live est. B/entry | data / index / filter B/entry | load ms | compact ms | 20000 gets ms | scan ms |
|---|---:|---:|---:|---:|---|---:|---:|---:|---:|
| uncompressed | 217.7 | 217.7 | — | 217.7 | 213.7 / 2.5 / 1.5 | 672 | 178 | 89 | 27 |
| node | 155.8 | 136.6 | +0.0% | 136.6 | 134.4 / 2.5 / 1.5 | 426 | 749 | 209 | 61 |
| previous (zstd3, keep >=12.5%) | 155.8 | 140.4 | +2.8% | 140.4 | 138.2 / 2.5 / 1.5 | 420 | 467 | 174 | 49 |
| previous + zstd9 | 155.8 | 138.1 | +1.1% | 138.1 | 135.9 / 2.5 / 1.5 | 520 | 719 | 197 | 51 |
| previous + keep >=0.8% | 155.8 | 139.3 | +2.0% | 139.3 | 137.1 / 2.5 / 1.5 | 615 | 496 | 201 | 60 |
| node, keep any saving | 155.8 | 136.6 | +0.0% | 136.6 | 134.4 / 2.5 / 1.5 | 477 | 735 | 220 | 62 |
| node, zstd6 | 155.8 | 136.8 | +0.1% | 136.8 | 134.5 / 2.5 / 1.5 | 452 | 576 | 195 | 60 |
| node, zstd19 | 155.8 | 134.6 | -1.5% | 134.6 | 132.4 / 2.5 / 1.5 | 462 | 2661 | 250 | 63 |
| node, no dictionary | 155.8 | 142.2 | +4.1% | 142.2 | 140.2 / 2.5 / 1.5 | 496 | 476 | 113 | 35 |
| node, 64KB dictionary | 155.8 | 135.7 | -0.6% | 135.7 | 133.0 / 2.5 / 1.5 | 392 | 1091 | 211 | 61 |
| node, restart interval 32 | 155.6 | 136.3 | -0.2% | 136.3 | 134.1 / 2.5 / 1.5 | 492 | 783 | 233 | 61 |
| node, separate keys and values | 156.2 | 136.0 | -0.4% | 136.0 | 133.8 / 2.5 / 1.5 | 473 | 699 | 240 | 59 |
| node, 8KB blocks | 152.8 | 134.5 | -1.5% | 134.5 | 132.6 / 1.2 / 1.5 | 540 | 648 | 245 | 50 |
| node, 16KB blocks | 150.8 | 133.1 | -2.6% | 133.1 | 131.3 / 0.6 / 1.5 | 485 | 635 | 302 | 34 |
| node, 64KB blocks | 148.3 | 131.1 | -4.0% | 131.1 | 129.4 / 0.2 / 1.5 | 437 | 495 | 668 | 27 |
| node, ribbon filter | 155.4 | 136.2 | -0.3% | 136.2 | 134.4 / 2.5 / 1.0 | 551 | 711 | 189 | 60 |
| node, no bottommost filter | 155.8 | 135.1 | -1.1% | 135.1 | 134.4 / 2.5 / 0.0 | 438 | 682 | 183 | 60 |
| node, 8KB blocks + ribbon | 152.4 | 134.1 | -1.8% | 134.1 | 132.6 / 1.2 / 1.0 | 390 | 611 | 223 | 43 |

## chat — 4 contexts, 40000 entries, 56004 rows, 1.4 rows/entry, logical 380 B/entry (keys 91 + values 289)

| variant | flushed B/entry | compacted B/entry | vs node | live est. B/entry | data / index / filter B/entry | load ms | compact ms | 20000 gets ms | scan ms |
|---|---:|---:|---:|---:|---|---:|---:|---:|---:|
| uncompressed | 359.8 | 359.8 | — | 359.8 | 353.8 / 4.2 / 1.8 | 207 | 48 | 72 | 15 |
| node | 219.0 | 159.9 | +0.0% | 159.9 | 156.8 / 4.2 / 1.8 | 186 | 528 | 178 | 36 |
| previous (zstd3, keep >=12.5%) | 224.1 | 172.0 | +7.6% | 172.0 | 168.9 / 4.2 / 1.8 | 202 | 306 | 152 | 31 |
| previous + zstd9 | 224.1 | 167.8 | +4.9% | 167.8 | 164.6 / 4.2 / 1.8 | 214 | 516 | 157 | 32 |
| previous + keep >=0.8% | 219.0 | 162.8 | +1.8% | 162.8 | 159.6 / 4.2 / 1.8 | 235 | 302 | 179 | 36 |
| node, keep any saving | 219.0 | 159.9 | +0.0% | 159.9 | 156.8 / 4.2 / 1.8 | 215 | 602 | 201 | 37 |
| node, zstd6 | 219.0 | 162.3 | +1.5% | 162.3 | 159.1 / 4.2 / 1.8 | 291 | 446 | 197 | 50 |
| node, zstd19 | 219.0 | 157.0 | -1.8% | 157.0 | 153.9 / 4.2 / 1.8 | 284 | 2021 | 185 | 37 |
| node, no dictionary | 219.0 | 192.0 | +20.1% | 192.0 | 189.3 / 4.2 / 1.8 | 231 | 343 | 144 | 29 |
| node, 64KB dictionary | 219.0 | 158.1 | -1.1% | 158.1 | 153.8 / 4.2 / 1.8 | 223 | 1029 | 253 | 37 |
| node, restart interval 32 | 218.9 | 159.7 | -0.1% | 159.7 | 156.6 / 4.2 / 1.8 | 368 | 622 | 197 | 36 |
| node, separate keys and values | 219.1 | 159.0 | -0.6% | 159.0 | 155.9 / 4.2 / 1.8 | 262 | 564 | 182 | 37 |
| node, 8KB blocks | 203.4 | 155.7 | -2.6% | 155.7 | 153.0 / 2.1 / 1.8 | 193 | 531 | 225 | 26 |
| node, 16KB blocks | 195.1 | 153.8 | -3.8% | 153.8 | 151.4 / 1.0 / 1.8 | 272 | 504 | 294 | 20 |
| node, 64KB blocks | 187.9 | 152.0 | -5.0% | 152.0 | 149.7 / 0.3 / 1.8 | 219 | 374 | 647 | 15 |
| node, ribbon filter | 218.5 | 159.4 | -0.3% | 159.4 | 156.8 / 4.2 / 1.2 | 233 | 522 | 181 | 37 |
| node, no bottommost filter | 219.0 | 158.2 | -1.1% | 158.2 | 156.8 / 4.2 / 0.0 | 216 | 537 | 176 | 36 |
| node, 8KB blocks + ribbon | 202.9 | 155.2 | -3.0% | 155.2 | 153.0 / 2.1 / 1.2 | 241 | 478 | 213 | 26 |

`previous` is the node before this tool: bottommost ZSTD level 3, and
RocksDB's default of storing a block raw unless compression saves 12.5%.

## Reading the numbers

- **Key overhead is already small.** Delta encoding stores the 33 bytes shared with the previous key once per restart interval. So a row costs roughly its 31 random id bytes, 8 bytes of sequence and type, and three varints. Restart interval 32 saves 0.2%.
- **Random bytes set the floor.** Each kv entry carries about 96 incompressible bytes: its 32-byte id in the key, and the `id ‖ hash` slot its parent's child trie keeps for it. Compressing the sorted row stream as one whole file with `xz -9e` gets to 94 B per kv entry and 113 B per message. No block-level codec gets near that, because each 4KB block is compressed on its own.
- **The dictionary is the largest single win.** Without it, chat state is 20% larger.
- **The compression threshold mattered most among the knobs tried.** Many state blocks compress by less than 12.5%, and with RocksDB's default threshold every such block was stored raw. Keeping a block whenever it saves at least 0.8% cuts state by about 1% (kv) and 5% (chat). That block then costs a decompression on a cache miss: cold point reads are 15-25% slower in the table above. Warm reads are unaffected, because the block cache holds blocks uncompressed.
- **ZSTD level 9 is the cheap part of the level curve.** It is 1.7-3.1% smaller than level 3 for about 1.6x the bottommost compaction CPU, and reads are unchanged. Level 19 saves another 1.5% for about 4x the compaction CPU.
- **Larger blocks trade cold reads for size.** 16KB blocks would save another 2.5-3.8%, but cold point reads get about 1.2-1.6x slower than the node's, and 64KB blocks make them about 3x slower. They are not adopted because every storage read from the guest is a point read.
- **Small, risky or free-but-tiny options are not adopted:**
  - dropping the bottommost filter saves 1.1%, but every read of an absent key then touches a data block;
  - a ribbon filter saves 0.3%;
  - separating keys from values in data blocks changes nothing measurable.
- **The prefix extractor is not a size lever.** A 32-byte context prefix extractor only adds prefix entries to the filter; delta encoding already removes the shared prefix from the data blocks.

None of the adopted options change readability. Compression level and the
threshold apply when a file is written, so an existing database opens unchanged
and its files take the new settings when compaction next rewrites them.
`format_version` is already RocksDB 11's default (7, readable by RocksDB ≥ 10.4).
