//! What does one key-value operation cost, and does it get worse as the
//! database grows?
//!
//! Two backends, deliberately: the in-memory DB is the abstraction floor (no
//! I/O, so what remains is codec and dispatch), and RocksDB on a tmpdir is the
//! dispatch-plus-warm-cache cost a node pays on top of that floor.
//!
//! # Cache-resident vs. cache-evicting (2026-08-31 measurement)
//!
//! The original version of this file measured ONLY at `n <= 10_000`, where
//! the ~2 MB working set sits three orders of magnitude under RocksDB's
//! 128 MiB default block cache (`DEFAULT_BLOCK_CACHE_SIZE`,
//! `crates/store/impl/rocksdb/src/lib.rs`) — every `get_hit` after the first
//! touch was a cache hit, not a disk read, so that sweep could not answer
//! "does read cost degrade as the database grows".
//!
//! This file now runs the SAME `n` sweep twice against RocksDB: once against
//! the real 128 MiB default (`rocks`, cache-resident at every size here —
//! growing `n` to actually exceed 128 MiB would need ~760,000 rows at
//! ~176 bytes/row, an order of magnitude past what this bench affords to
//! populate on every CI run), and once with the block cache override
//! (`CALIMERO_ROCKSDB_BLOCK_CACHE_BYTES_FOR_BENCH`, read once at `open()`,
//! unset in every real deployment — see `crates/store/impl/rocksdb/src`) shrunk to
//! `EVICTING_CACHE_BYTES`, deliberately below the smallest benchmarked
//! working set, so `get_hit` there is answered from an evicted cache at
//! EVERY `n` — the honest way to ask "does a cache MISS get more expensive
//! as the database grows" without needing gigabytes of tmpdir. See
//! `docs/benchmarking.md`'s RocksDB section (or the final-measurements
//! report this bench feeds) for the resulting curve and what it says about
//! `get_hit`'s claimed cache-residency.
//!
//! `rocks` (default cache) is NOT a cold-disk-seek number and still is not
//! one after this change — it says nothing about disk I/O, only about
//! dispatch-plus-warm-cache cost. `rocks_evicting` is the number that
//! actually answers the degradation question, at the cost of also paying
//! FFI/dispatch overhead on every operation (it is eviction-forced, not a
//! clean isolation of ONLY the disk-seek cost).
//!
//! `read_then_put` is the merge-path pattern — read the existing value, write
//! a new one under the same key — which is what a delta apply does per entity.
//!
//! Population accounting: each group is populated with exactly `n` distinct
//! keys before any measurement starts, and the database never grows past `n`
//! for the rest of the group. `get_hit` and `get_miss` never mutate the
//! database. `read_then_put` reads and rewrites `hit_keys` — the same 32 keys
//! `get_hit` reads — in place, one key per iteration. `put` cycles through a
//! fixed 128-element `put_keys` vector, overwriting the same 128 keys on
//! every iteration once the cursor wraps. So both mutators hold the database
//! at exactly `n` rows for the whole group; none of the four benchmarked ops
//! ever grows it.
//!
//! What would change a decision: at these sizes the whole column family fits
//! in the block cache, so a `get_hit` that climbs with `n` cannot be
//! compaction/SST-growth pressure (that needs the cache exceeded first, which
//! none of the benchmarked `n` do) — it would instead point at per-entry
//! dispatch cost scaling with database size, worth chasing in
//! `crates/store`'s own code before looking a layer down.

use std::hint::black_box;

use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database, InMemoryDB};
use calimero_store::slice::Slice;
use calimero_store_rocksdb::RocksDB;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use tempfile::TempDir;

const KEY_LEN: usize = 48;
const VALUE_LEN: usize = 128;
const COLUMN: Column = Column::Generic;

/// Distinct keys spread across the keyspace, so RocksDB's seeks are not all
/// answered from one hot SST block.
fn key_bytes(i: u64) -> [u8; KEY_LEN] {
    let mut k = [0_u8; KEY_LEN];
    k[..8].copy_from_slice(&i.to_le_bytes());
    k[8..16].copy_from_slice(&i.wrapping_mul(2_654_435_761).to_le_bytes());
    k[16..24].copy_from_slice(&i.wrapping_mul(6_364_136_223_846_793_005).to_le_bytes());
    k
}

/// 128 bytes — a small delta record after borsh encoding, not a multi-KB root
/// state.
fn value_bytes(i: u64) -> [u8; VALUE_LEN] {
    let mut v = [0_u8; VALUE_LEN];
    v[..8].copy_from_slice(&i.to_le_bytes());
    v[VALUE_LEN - 8..].copy_from_slice(&i.wrapping_mul(6_364_136_223_846_793_005).to_le_bytes());
    v
}

fn populate<D: for<'a> Database<'a>>(db: &D, n: usize) {
    for i in 0..n as u64 {
        let k = key_bytes(i);
        let v = value_bytes(i);
        db.put(COLUMN, Slice::from(&k[..]), Slice::from(&v[..]))
            .expect("populating put must succeed");
    }
}

fn run_group<D: for<'a> Database<'a>>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    db: &D,
    n: usize,
) {
    let hit_keys: Vec<[u8; KEY_LEN]> = (0..32_u64)
        .map(|i| i * (n as u64 / 32).max(1))
        .map(key_bytes)
        .collect();
    let miss_keys: Vec<[u8; KEY_LEN]> = (0..32_u64).map(|i| key_bytes(n as u64 + 1 + i)).collect();
    let put_keys: Vec<[u8; KEY_LEN]> = (0..128_u64)
        .map(|i| key_bytes(n as u64 + 10_000 + i))
        .collect();
    let payload = value_bytes(42);

    group.bench_function(BenchmarkId::new("get_hit", n), |b| {
        let mut cursor = 0_usize;
        b.iter(|| {
            cursor = cursor.wrapping_add(1);
            let k = &hit_keys[cursor % hit_keys.len()];
            black_box(
                db.get(COLUMN, Slice::from(&k[..]))
                    .expect("get must succeed"),
            )
        });
    });

    group.bench_function(BenchmarkId::new("get_miss", n), |b| {
        let mut cursor = 0_usize;
        b.iter(|| {
            cursor = cursor.wrapping_add(1);
            let k = &miss_keys[cursor % miss_keys.len()];
            black_box(
                db.get(COLUMN, Slice::from(&k[..]))
                    .expect("get must succeed"),
            )
        });
    });

    group.bench_function(BenchmarkId::new("put", n), |b| {
        let mut cursor = 0_usize;
        b.iter(|| {
            cursor = cursor.wrapping_add(1);
            let k = &put_keys[cursor % put_keys.len()];
            db.put(COLUMN, Slice::from(&k[..]), Slice::from(&payload[..]))
                .expect("put must succeed");
        });
    });

    group.bench_function(BenchmarkId::new("read_then_put", n), |b| {
        let mut cursor = 0_usize;
        b.iter(|| {
            cursor = cursor.wrapping_add(1);
            let k = &hit_keys[cursor % hit_keys.len()];
            let prev = db
                .get(COLUMN, Slice::from(&k[..]))
                .expect("get must succeed");
            black_box(&prev);
            let v = value_bytes(cursor as u64);
            db.put(COLUMN, Slice::from(&k[..]), Slice::from(&v[..]))
                .expect("put must succeed");
        });
    });
}

fn inmem(c: &mut Criterion) {
    let mut group = c.benchmark_group("inmem");
    for n in [100_usize, 1_000, 10_000] {
        let db = InMemoryDB::owned();
        populate(&db, n);
        run_group(&mut group, &db, n);
    }
    group.finish();
}

fn rocks(c: &mut Criterion) {
    let mut group = c.benchmark_group("rocks");
    for n in [100_usize, 1_000, 10_000] {
        let dir = TempDir::new().expect("tempdir must create");
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("tempdir path must be utf-8");
        let db = RocksDB::open(&StoreConfig::new(path)).expect("rocksdb must open");
        populate(&db, n);
        run_group(&mut group, &db, n);
        // `dir` drops here, taking the database with it.
    }
    group.finish();
}

/// Block cache size (bytes) used for the `rocks_evicting` group: below the
/// ~176-bytes/row working set at even the smallest benchmarked `n = 100`
/// (~17.6 KB), so `get_hit` cannot be answered from a warm cache at ANY `n`
/// in this sweep — every read is forced to miss the block cache and go
/// through RocksDB's own (still in-process-page-cache-backed on most CI
/// runners) block read path, not the trivial in-cache lookup the default-size
/// `rocks` group measures.
const EVICTING_CACHE_BYTES: &str = "4096";

/// Env var `crates/store/impl/rocksdb/src/lib.rs` reads once at `open()` to
/// override the block cache size — unset in every real deployment.
const BLOCK_CACHE_ENV: &str = "CALIMERO_ROCKSDB_BLOCK_CACHE_BYTES_FOR_BENCH";

/// Same sweep as `rocks`, with the block cache shrunk far below the working
/// set so every `get_hit` is a forced cache miss at every `n` — see the
/// module doc's "Cache-resident vs. cache-evicting" section for why the
/// `rocks` group above cannot answer the read-degradation question on its
/// own.
fn rocks_evicting(c: &mut Criterion) {
    // SAFETY: this bench binary is single-threaded at this point (no other
    // thread has been spawned yet, and criterion's `iter` closures below run
    // on this same thread) — `RocksDB::open` is the only reader of this var,
    // called synchronously per group iteration, so there is no concurrent
    // read/write race despite `set_var`/`remove_var` being marked `unsafe`
    // as of the 2024 edition's env-mutation audit.
    unsafe {
        std::env::set_var(BLOCK_CACHE_ENV, EVICTING_CACHE_BYTES);
    }

    let mut group = c.benchmark_group("rocks_evicting");
    for n in [100_usize, 1_000, 10_000] {
        let dir = TempDir::new().expect("tempdir must create");
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("tempdir path must be utf-8");
        let db = RocksDB::open(&StoreConfig::new(path)).expect("rocksdb must open");
        populate(&db, n);
        run_group(&mut group, &db, n);
        // `dir` drops here, taking the database with it.
    }
    group.finish();

    // SAFETY: same single-threaded reasoning as the `set_var` above; this
    // runs after every `RocksDB::open` call this function makes, so clearing
    // it here cannot race a still-pending read of it.
    unsafe {
        std::env::remove_var(BLOCK_CACHE_ENV);
    }
}

criterion_group!(benches, inmem, rocks, rocks_evicting);
criterion_main!(benches);
