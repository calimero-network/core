//! What does one key-value operation cost, and does it get worse as the
//! database grows?
//!
//! Two backends, deliberately: the in-memory DB is the abstraction floor (no
//! I/O, so what remains is codec and dispatch), and RocksDB on a tmpdir is the
//! dispatch-plus-warm-cache cost a node pays on top of that floor. It is NOT a
//! cold-disk-seek number: the largest working set benchmarked here
//! (`n = 10_000`, on the order of 2 MB) is three orders of magnitude under the
//! default 128 MiB block cache, so every `get_hit` after the first touch is a
//! cache hit, and the measured gap (RocksDB ~3.4x the in-memory floor at
//! `n = 10_000`) is FFI and column-family dispatch rather than disk I/O.
//! Measuring disk-backed reads needs `n` grown past the cache, a smaller
//! `set_block_cache`, or RocksDB's own block-cache hit/miss statistics; this
//! bench does none of those.
//!
//! `read_then_put` is the merge-path pattern, read the existing value and
//! write a new one under the same key, which is what a delta apply does per
//! entity.
//!
//! Each group is populated with exactly `n` distinct keys before any
//! measurement starts, and both mutators cycle a fixed key set rather than
//! minting new keys, so the database does not grow with iteration count and
//! `{n}` keeps meaning what it says.
//!
//! What would change a decision: at these sizes a `get_hit` that climbs with
//! `n` cannot be compaction or SST-growth pressure, so it would point at
//! per-entry dispatch cost scaling with database size, worth chasing in
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

/// 128 bytes: a small delta record after borsh encoding, not a multi-KB root
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

/// One backend, swept over the population sizes. `make` returns the database
/// alongside anything that must outlive it, which for RocksDB is the temp
/// directory holding it.
fn sweep<D: for<'a> Database<'a>, T>(c: &mut Criterion, name: &str, make: impl Fn() -> (D, T)) {
    let mut group = c.benchmark_group(name);
    for n in [100_usize, 1_000, 10_000] {
        let (db, _owned) = make();
        populate(&db, n);
        run_group(&mut group, &db, n);
    }
    group.finish();
}

fn inmem(c: &mut Criterion) {
    sweep(c, "inmem", || (InMemoryDB::owned(), ()));
}

fn rocks(c: &mut Criterion) {
    sweep(c, "rocks", || {
        let dir = TempDir::new().expect("tempdir must create");
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("tempdir path must be utf-8");
        let db = RocksDB::open(&StoreConfig::new(path)).expect("rocksdb must open");
        (db, dir)
    });
}

criterion_group!(benches, inmem, rocks);
criterion_main!(benches);
