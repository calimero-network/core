//! What does one key-value operation cost, and does it get worse as the
//! database grows?
//!
//! Two backends, deliberately: the in-memory DB is the abstraction floor (no
//! I/O, so what remains is codec and dispatch), and RocksDB on a tmpdir is the
//! cost a node actually pays. The gap between them is the I/O budget of every
//! storage read in the tree.
//!
//! `read_then_put` is the merge-path pattern — read the existing value, write
//! a new one under the same key — which is what a delta apply does per entity.
//!
//! Population accounting: each group id `{inmem,rocks}/{n}` is populated with
//! exactly `n` distinct keys before any measurement starts. `get_hit` and
//! `get_miss` never mutate the database, so the size stays exactly `n` for the
//! whole group. `put` and `read_then_put` write into a disjoint key range set
//! aside above `n` (`n + 10_000 ..`), so thousands of criterion samples do grow
//! the database past `n` — but they never touch the `n`-sized keyspace that
//! `get_hit`/`get_miss` read from within the same group, so those two ids keep
//! measuring exactly the database size their group id claims.
//!
//! What would change a decision: `get_hit` on RocksDB that climbs with `n`
//! points at compaction/SST growth rather than at anything in `crates/storage`,
//! and moves the investigation a layer down.

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

fn run_group<D: for<'a> Database<'a>>(c: &mut Criterion, prefix: &str, db: &D, n: usize) {
    let mut group = c.benchmark_group(prefix);

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

    group.finish();
}

fn inmem(c: &mut Criterion) {
    for n in [100_usize, 1_000, 10_000] {
        let db = InMemoryDB::owned();
        populate(&db, n);
        run_group(c, &format!("inmem/{n}"), &db, n);
    }
}

fn rocks(c: &mut Criterion) {
    for n in [100_usize, 1_000, 10_000] {
        let dir = TempDir::new().expect("tempdir must create");
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("tempdir path must be utf-8");
        let db = RocksDB::open(&StoreConfig::new(path)).expect("rocksdb must open");
        populate(&db, n);
        run_group(c, &format!("rocks/{n}"), &db, n);
        // `dir` drops here, taking the database with it.
    }
}

criterion_group!(benches, inmem, rocks);
criterion_main!(benches);
