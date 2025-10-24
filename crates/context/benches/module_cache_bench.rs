use std::sync::Arc;

use calimero_context::compiled_module_cache::CompiledModuleCache;
use calimero_primitives::blobs::BlobId;
use calimero_runtime::Engine;
use calimero_store::{config::StoreConfig, key, slice::Slice, Store};
use calimero_store_rocksdb::RocksDB;
use camino::Utf8PathBuf;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tempfile::TempDir;

// Sample WASM module - use the kv_store example
const WASM_BYTES: &[u8] = include_bytes!("../../../apps/kv-store/res/kv_store.wasm");

fn bench_cache_tiers(c: &mut Criterion) {
    let mut group = c.benchmark_group("module_loading");

    // Setup real RocksDB for realistic benchmarks
    let temp_dir = TempDir::new().unwrap();
    let store = Store::open::<RocksDB>(&StoreConfig::new(
        Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap(),
    ))
    .unwrap();

    let engine = Engine::default();
    let cache = Arc::new(CompiledModuleCache::default());

    // Compile once and store in RocksDB
    let module = engine.compile(WASM_BYTES).unwrap();
    let compiled_bytes = module.to_bytes().unwrap();

    // Store compiled module in real RocksDB using Generic key
    let blob_id = BlobId::from([1u8; 32]);
    let mut scope = [0u8; 16];
    scope.copy_from_slice(b"compiled_modules");
    let key = key::Generic::new(scope, *blob_id.as_ref());

    {
        let mut handle = store.handle();
        let value = Slice::from(&*compiled_bytes).into();
        handle.put(&key, &value).unwrap();
    }

    // Tier 1: LRU cache hit (fastest - pure memory access)
    group.bench_function("1_lru_cache_hit", |b| {
        cache.put(blob_id, compiled_bytes.clone());

        b.iter(|| {
            let cached = cache.get(black_box(&blob_id)).unwrap();
            unsafe { Engine::headless().from_precompiled(&cached).unwrap() }
        });
    });

    // Tier 2: REAL RocksDB read (what happens on cache miss)
    group.bench_function("2_rocksdb_read", |b| {
        cache.clear();

        b.iter(|| {
            // Real RocksDB read with disk I/O
            let handle = store.handle();
            let value = handle.get(&key).unwrap().expect("Value not found");
            let bytes: Box<[u8]> = value.as_ref().to_vec().into_boxed_slice();

            let module = unsafe { Engine::headless().from_precompiled(&bytes).unwrap() };
            cache.put(blob_id, bytes);
            module
        });
    });

    // Tier 3: Full compilation (slowest - compile from source)
    group.bench_function("3_full_compilation", |b| {
        let mut counter = 0u8;

        b.iter(|| {
            let module = engine.compile(black_box(WASM_BYTES)).unwrap();
            let bytes = module.to_bytes().unwrap();

            // Store in RocksDB + cache with unique key
            counter = counter.wrapping_add(1);
            let new_blob_id = BlobId::from([counter; 32]);
            let new_key = key::Generic::new(scope, *new_blob_id.as_ref());

            // Cache first (takes ownership)
            cache.put(new_blob_id, bytes.clone());

            // Then store in RocksDB
            let mut handle = store.handle();
            let value = Slice::from(&*bytes).into();
            handle.put(&new_key, &value).unwrap();

            module
        });
    });

    group.finish();
}

fn bench_cache_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_operations");

    let cache = Arc::new(CompiledModuleCache::default());
    let engine = Engine::default();
    let module = engine.compile(WASM_BYTES).unwrap();
    let compiled_bytes = module.to_bytes().unwrap();
    let blob_id = BlobId::from([1u8; 32]);

    // Cache get (hit)
    group.bench_function("get_hit", |b| {
        cache.put(blob_id, compiled_bytes.clone());

        b.iter(|| cache.get(black_box(&blob_id)));
    });

    // Cache get (miss)
    group.bench_function("get_miss", |b| {
        cache.clear();

        b.iter(|| cache.get(black_box(&blob_id)));
    });

    // Cache put
    group.bench_function("put", |b| {
        b.iter(|| {
            let id = BlobId::from(black_box([2u8; 32]));
            cache.put(id, compiled_bytes.clone());
        });
    });

    group.finish();
}

fn bench_hot_contract_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("hot_contract_simulation");
    group.sample_size(10); // Reduce samples since this is expensive

    // Setup real RocksDB
    let temp_dir = TempDir::new().unwrap();
    let store = Store::open::<RocksDB>(&StoreConfig::new(
        Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap(),
    ))
    .unwrap();

    let engine = Engine::default();
    let cache = Arc::new(CompiledModuleCache::default());

    // Pre-compile and store
    let module = engine.compile(WASM_BYTES).unwrap();
    let compiled_bytes = module.to_bytes().unwrap();
    let blob_id = BlobId::from([1u8; 32]);
    let mut scope = [0u8; 16];
    scope.copy_from_slice(b"compiled_modules");
    let key = key::Generic::new(scope, *blob_id.as_ref());

    {
        let mut handle = store.handle();
        let value = Slice::from(&*compiled_bytes).into();
        handle.put(&key, &value).unwrap();
    }

    // Simulate 100 executions WITH LRU cache
    group.bench_function("100_executions_with_lru", |b| {
        b.iter(|| {
            cache.clear();

            // First execution: RocksDB read + cache
            let handle = store.handle();
            let value = handle.get(&key).unwrap().unwrap();
            let bytes: Box<[u8]> = value.as_ref().to_vec().into_boxed_slice();
            cache.put(blob_id, bytes.clone());
            let _module = unsafe { Engine::headless().from_precompiled(&bytes).unwrap() };

            // Next 99 executions: LRU cache hits (no RocksDB!)
            for _ in 0..99 {
                let cached = cache.get(&blob_id).unwrap();
                let _module = unsafe { Engine::headless().from_precompiled(&cached).unwrap() };
            }
        });
    });

    // WITHOUT LRU cache - every execution reads from RocksDB
    group.bench_function("100_executions_without_lru", |b| {
        b.iter(|| {
            // All 100 executions: RocksDB read every time
            for _ in 0..100 {
                let handle = store.handle();
                let value = handle.get(&key).unwrap().unwrap();
                let bytes: Box<[u8]> = value.as_ref().to_vec().into_boxed_slice();
                let _module = unsafe { Engine::headless().from_precompiled(&bytes).unwrap() };
            }
        });
    });

    group.finish();
}

fn bench_cache_size_impact(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_size");

    let engine = Engine::default();
    let module = engine.compile(WASM_BYTES).unwrap();
    let compiled_bytes = module.to_bytes().unwrap();

    for size in [1, 8, 32, 64] {
        group.bench_function(format!("size_{}", size), |b| {
            let cache = Arc::new(CompiledModuleCache::new(size));

            b.iter(|| {
                // Simulate accessing different contracts
                for i in 0..size {
                    let blob_id = BlobId::from([i as u8; 32]);
                    cache.put(blob_id, compiled_bytes.clone());
                }

                // Access them all (should all be in cache)
                for i in 0..size {
                    let blob_id = BlobId::from([i as u8; 32]);
                    let _ = cache.get(&blob_id);
                }
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_cache_tiers,
    bench_cache_operations,
    bench_hot_contract_execution,
    bench_cache_size_impact
);
criterion_main!(benches);
