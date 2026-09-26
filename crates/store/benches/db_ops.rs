//! What does one key-value operation cost, and does it get worse as the
//! database grows? RocksDB numbers here are warm-cache, not disk-seek.

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

fn key_bytes(i: u64) -> [u8; KEY_LEN] {
    let mut k = [0_u8; KEY_LEN];
    k[..8].copy_from_slice(&i.to_le_bytes());
    k[8..16].copy_from_slice(&i.wrapping_mul(2_654_435_761).to_le_bytes());
    k[16..24].copy_from_slice(&i.wrapping_mul(6_364_136_223_846_793_005).to_le_bytes());
    k
}

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
}

// `make`'s second return must outlive the database: RocksDB's temp directory.
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
