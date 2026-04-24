//! Micro-benchmarks for `Database::get` / `Database::put` at varying
//! database sizes, across both in-memory and RocksDB backends.
//!
//! Targets issue #2199 suspect (a) from PR #2196's investigation:
//! `try_merge_data` in `crates/storage/src/interface.rs` does an extra
//! `storage_read(Key::Entry(id))` before merging the existing root
//! state with the incoming delta. The 918ms apply-latency outlier
//! wasn't explained by the merkle rehash (~17ms at N=100k children)
//! or `merge_root_state` framework overhead (~18µs at N=1k items).
//! If the culprit is RocksDB read latency in that extra storage hit —
//! under concurrent compaction, cold cache, or large DB state — a
//! direct `get` bench will surface it.
//!
//! The in-memory backend shows the abstraction floor (no disk I/O);
//! RocksDB on a tmp-dir path shows realistic production read cost.
//! Benchmarks bypass the `Handle<...>` typed codec layer and call
//! `Database::put` / `Database::get` directly, so measurements reflect
//! the actual read/write path rather than codec overhead.
//!
//! # What's measured
//!
//! - `get_hit`: read an existing key. The hot path for
//!   `try_merge_data`'s extra read.
//! - `get_miss`: read a non-existent key. Baseline negative-path cost
//!   (bloom filters should make this cheap on RocksDB).
//! - `put`: write a key. For comparison with read latency.
//! - `read_then_put`: the actual merge-path pattern — read an
//!   existing value, write a new one under the same key.
//!
//! All four sweep the database's pre-populated size. If read cost
//! scales (RocksDB compaction or SST growth making each read
//! progressively slower), we should see it here.
//!
//! # Running
//!
//! ```
//! cargo bench -p calimero-store --bench handle_ops
//! ```

use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database, InMemoryDB};
use calimero_store::slice::Slice;
use calimero_store_rocksdb::RocksDB;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use tempfile::TempDir;

const KEY_LEN: usize = 48;
const VALUE_LEN: usize = 128;
const COLUMN: Column = Column::Generic;

/// Produces a deterministic-but-distinct key for index `i`. Keys spread
/// across the fragment so they don't cluster at one end of the
/// keyspace — avoids biasing RocksDB's seek behaviour.
fn key_bytes(i: u64) -> [u8; KEY_LEN] {
    let mut k = [0u8; KEY_LEN];
    k[..8].copy_from_slice(&i.to_le_bytes());
    k[8..16].copy_from_slice(&i.wrapping_mul(2654435761).to_le_bytes());
    k[16..24].copy_from_slice(&i.wrapping_mul(6364136223846793005).to_le_bytes());
    k
}

/// 128 bytes — representative of a small delta record after borsh
/// encoding (not the big multi-KB root-state entries).
fn value_bytes(i: u64) -> [u8; VALUE_LEN] {
    let mut v = [0u8; VALUE_LEN];
    v[..8].copy_from_slice(&i.to_le_bytes());
    v[VALUE_LEN - 8..].copy_from_slice(&i.wrapping_mul(6364136223846793005).to_le_bytes());
    v
}

fn populate(db: &dyn for<'a> Database<'a>, n: usize) {
    for i in 0..n as u64 {
        let k = key_bytes(i);
        let v = value_bytes(i);
        db.put(COLUMN, Slice::from(&k[..]), Slice::from(&v[..]))
            .expect("populate put should succeed");
    }
}

fn bench_inmem(c: &mut Criterion) {
    for n in [100usize, 1_000, 10_000] {
        let db = InMemoryDB::owned();
        populate(&db, n);
        run_group(c, &format!("inmem/{}", n), &db, n);
    }
}

fn bench_rocks(c: &mut Criterion) {
    for n in [100usize, 1_000, 10_000] {
        let dir = TempDir::new().expect("tempdir should create");
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("tempdir path should be utf-8");
        let config = StoreConfig::new(path);
        let db = RocksDB::open(&config).expect("rocksdb open should succeed");
        populate(&db, n);
        run_group(c, &format!("rocks/{}", n), &db, n);
        // `dir` drops here → tempdir cleaned up.
    }
}

fn run_group(c: &mut Criterion, prefix: &str, db: &dyn for<'a> Database<'a>, n: usize) {
    let mut group = c.benchmark_group(prefix);

    // A handful of in-range ids so criterion's warm-up stays
    // deterministic. Picking distinct ids per sample avoids measuring
    // only the L1-cache-hot case in memory and shows realistic cache
    // behaviour on RocksDB.
    let probe_keys: Vec<[u8; KEY_LEN]> = (0..32u64)
        .map(|i| i * (n as u64 / 32).max(1))
        .map(key_bytes)
        .collect();

    // --- get_hit -----------------------------------------------------
    group.bench_function(BenchmarkId::new("get_hit", n), |b| {
        let mut cursor = 0usize;
        b.iter(|| {
            let k = &probe_keys[cursor % probe_keys.len()];
            cursor = cursor.wrapping_add(1);
            let v = db
                .get(COLUMN, Slice::from(black_box(&k[..])))
                .expect("get should succeed");
            black_box(v);
        });
    });

    // --- get_miss ----------------------------------------------------
    let miss_keys: Vec<[u8; KEY_LEN]> = (0..32u64)
        .map(|i| key_bytes(n as u64 + 1 + i))
        .collect();
    group.bench_function(BenchmarkId::new("get_miss", n), |b| {
        let mut cursor = 0usize;
        b.iter(|| {
            let k = &miss_keys[cursor % miss_keys.len()];
            cursor = cursor.wrapping_add(1);
            let v = db
                .get(COLUMN, Slice::from(black_box(&k[..])))
                .expect("get should succeed");
            black_box(v);
        });
    });

    // --- put ---------------------------------------------------------
    // Writes land in a "past the populated range" slot so they don't
    // shift the populated-size gauge mid-bench.
    let put_keys: Vec<[u8; KEY_LEN]> = (0..128u64)
        .map(|i| key_bytes(n as u64 + 10_000 + i))
        .collect();
    let payload = value_bytes(42);
    group.bench_function(BenchmarkId::new("put", n), |b| {
        let mut cursor = 0usize;
        b.iter(|| {
            let k = &put_keys[cursor % put_keys.len()];
            cursor = cursor.wrapping_add(1);
            db.put(
                COLUMN,
                Slice::from(black_box(&k[..])),
                Slice::from(&payload[..]),
            )
            .expect("put should succeed");
        });
    });

    // --- read_then_put ----------------------------------------------
    // Simulates the `try_merge_data` pattern: read existing value,
    // compute-merge (no-op in this bench), write the result back.
    group.bench_function(BenchmarkId::new("read_then_put", n), |b| {
        let mut cursor = 0usize;
        b.iter(|| {
            let k = &probe_keys[cursor % probe_keys.len()];
            cursor = cursor.wrapping_add(1);
            let prev = db
                .get(COLUMN, Slice::from(black_box(&k[..])))
                .expect("get should succeed");
            black_box(&prev);
            let v = value_bytes(cursor as u64);
            db.put(
                COLUMN,
                Slice::from(black_box(&k[..])),
                Slice::from(&v[..]),
            )
            .expect("put should succeed");
        });
    });

    group.finish();
}

criterion_group!(benches, bench_inmem, bench_rocks);
criterion_main!(benches);
