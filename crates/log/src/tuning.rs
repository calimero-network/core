//! RocksDB tuning every node store opens with (moved from v1's
//! `calimero-store-rocksdb`, unchanged): LZ4 above the bottommost level, ZSTD
//! with a trained dictionary at it, bloom filters, early compaction of
//! deletion-heavy files.

use eyre::Result as EyreResult;
use rocksdb::{BlockBasedOptions, Cache, DBCompressionType, Options};

/// Bloom filter density: ~1% false positives for ~1.25 bytes per key.
///
/// Without a filter, a point lookup for an absent key reads a data block from
/// every level that might hold it, and `has()` can only short-circuit on keys
/// still in a memtable or the block cache.
const BLOOM_FILTER_BITS_PER_KEY: f64 = 10.0;

/// ZSTD level for the bottommost level, where ~90% of the data settles.
///
/// Measured with `tools/state-disk-cost` on real kv and chat state, level 9
/// against ZSTD's default 3 cuts the compacted SSTs by 1.7% (kv) and 3.1%
/// (chat) and costs about 1.6x the CPU in bottommost compactions; point reads
/// do not change, since ZSTD decompresses at the same speed whatever the
/// level. Level 19 bought only another 1.5% for 4x the compaction time.
const BOTTOMMOST_ZSTD_LEVEL: i32 = 9;

/// ZSTD window size for the bottommost level. `-14` is RocksDB's default.
const BOTTOMMOST_ZSTD_WINDOW_BITS: i32 = -14;

/// Size of the ZSTD dictionary trained per bottommost SST (16KB).
///
/// State rows are small and share structure (metadata layout, repeated ids),
/// which block-level compression alone cannot exploit across blocks; a shared
/// dictionary can.
const BOTTOMMOST_ZSTD_DICT_BYTES: i32 = 16 * 1024;

/// Sample size the dictionary is trained on: RocksDB's recommended 100x the
/// dictionary size.
const BOTTOMMOST_ZSTD_TRAIN_BYTES: i32 = BOTTOMMOST_ZSTD_DICT_BYTES * 100;

/// Keep a block compressed when that saves at least 8 bytes in every 1024.
///
/// RocksDB stores a block raw unless compression shrinks it by 1/8
/// (`max_compressed_bytes_per_kb` defaults to 896). State blocks are dense
/// with random ids and hashes, so many compress by less than that and were
/// stored raw. Keeping them cuts the compacted SSTs by another 1% (kv) and 5%
/// (chat) in `tools/state-disk-cost`. The price is a decompression on a block
/// cache miss for those blocks: cold point reads measured 15-25% slower, warm
/// ones are unchanged since the cache holds blocks uncompressed. Accepting any
/// saving at all (1023) measured the same as this; this bar only spares
/// blocks that would be decompressed to save almost nothing.
const MAX_COMPRESSED_BYTES_PER_KB: u32 = 1016;

/// Sliding window, in entries, that the deletion-triggered compaction collector
/// inspects.
const DELETION_COMPACTION_WINDOW: usize = 128 * 1024;

/// Deletions within [`DELETION_COMPACTION_WINDOW`] entries that mark an SST for
/// compaction.
const DELETION_COMPACTION_TRIGGER: usize = 16 * 1024;

/// Share of tombstones in an SST that marks it for compaction.
const DELETION_COMPACTION_RATIO: f64 = 0.5;

/// Options every column family is opened with.
///
/// Compression is LZ4 on every level but the bottommost, which uses ZSTD with a
/// trained dictionary: with dynamic level sizing (RocksDB's default) the
/// bottommost level holds most of the data, so that is where the stronger
/// codec pays for its CPU, while LZ4 keeps flushes and upper-level compactions
/// cheap.
///
/// Deletion-heavy files are compacted early so point deletes — tombstone GC,
/// delta pruning, trie rows dropped on delete — release their space without
/// waiting for the file to be picked up by size-triggered compaction.
///
/// Public so `tools/state-disk-cost` and `tools/storage-compare` measure
/// exactly what a node opens with.
///
/// # Errors
///
/// Only if RocksDB rejects the compression-threshold options string, which is
/// a constant: rust-rocksdb has no setter for `max_compressed_bytes_per_kb`.
pub fn column_options(table: &BlockBasedOptions) -> EyreResult<Options> {
    let mut options = Options::default();

    options.set_block_based_table_factory(table);

    options.set_compression_type(DBCompressionType::Lz4);
    options.set_bottommost_compression_type(DBCompressionType::Zstd);
    options.set_bottommost_compression_options(
        BOTTOMMOST_ZSTD_WINDOW_BITS,
        BOTTOMMOST_ZSTD_LEVEL,
        0,
        BOTTOMMOST_ZSTD_DICT_BYTES,
        true,
    );
    options.set_bottommost_zstd_max_train_bytes(BOTTOMMOST_ZSTD_TRAIN_BYTES, true);

    options.add_compact_on_deletion_collector_factory(
        DELETION_COMPACTION_WINDOW,
        DELETION_COMPACTION_TRIGGER,
        DELETION_COMPACTION_RATIO,
    );

    // Only the named fields change; the rest of each struct keeps what the
    // setters above put there.
    let options = options.get_options_from_string(format!(
        "compression_opts={{max_compressed_bytes_per_kb={MAX_COMPRESSED_BYTES_PER_KB}}};\
         bottommost_compression_opts={{max_compressed_bytes_per_kb={MAX_COMPRESSED_BYTES_PER_KB}}}"
    ))?;

    Ok(options)
}

/// Table options shared by every column family: one block cache for the whole
/// database, and a bloom filter per SST.
///
/// Index and filter blocks are charged to the block cache so their memory is
/// bounded by it rather than growing with the data, and L0's are pinned so the
/// files every read checks first never miss.
///
/// Public for the same reason as [`column_options`].
#[must_use]
pub fn table_options(cache: &Cache) -> BlockBasedOptions {
    let mut table = BlockBasedOptions::default();

    table.set_block_cache(cache);
    table.set_bloom_filter(BLOOM_FILTER_BITS_PER_KEY, false);
    table.set_cache_index_and_filter_blocks(true);
    table.set_pin_l0_filter_and_index_blocks_in_cache(true);

    table
}
