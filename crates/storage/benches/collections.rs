//! Criterion benchmarks for storage collection operations
//!
//! These benchmarks measure pure CRDT collection performance without WASM or runtime overhead.

use calimero_storage::collections::{
    GCounter, LwwRegister, ReplicatedGrowableArray, Root, UnorderedMap, UnorderedSet, Vector,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::path::Path;
use std::time::Duration;

mod common;


// Single-Threaded Benchmarks

/// Benchmark UnorderedMap insert operations
fn benchmark_storage_map_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_insert");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut map = Root::new(|| UnorderedMap::new());

            b.iter(|| {
                for i in 0..size {
                    let key = format!("key_{}", i);
                    let value = format!("value_{}", i);
                    black_box(map.insert(key, value).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedMap get operations
fn benchmark_storage_map_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_get");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Insert data first
            let mut map = Root::new(|| UnorderedMap::new());
            for i in 0..100 {
                let key = format!("key_{}", i);
                let value = format!("value_{}", i);
                map.insert(key, value).unwrap();
            }

            b.iter(|| {
                for i in 0..size {
                    let key = format!("key_{}", i % 100);
                    black_box(map.get(&key).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedMap remove operations
fn benchmark_storage_map_remove(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_remove");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Insert data first
            let mut map = Root::new(|| UnorderedMap::new());
            for i in 0..size {
                let key = format!("key_{}", i);
                let value = format!("value_{}", i);
                map.insert(key, value).unwrap();
            }

            b.iter(|| {
                // Re-insert after each iteration
                for i in 0..size {
                    let key = format!("key_{}", i);
                    let value = format!("value_{}", i);
                    map.insert(key.clone(), value).unwrap();
                    black_box(map.remove(&key).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark nested map insert operations (2 levels)
fn benchmark_storage_nested_map_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_nested_map_insert");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut map = Root::new(|| UnorderedMap::<String, UnorderedMap<String, String>>::new());

            b.iter(|| {
                for i in 0..size {
                    let outer_key = format!("outer_{}", i);
                    let inner_key = format!("inner_{}", i);
                    let value = format!("value_{}", i);

                    // Get or create inner map
                    let mut inner_map = map
                        .entry(outer_key)
                        .unwrap()
                        .or_insert_with(|| UnorderedMap::new())
                        .unwrap();
                    black_box(inner_map.insert(inner_key, value).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark deep nested map insert operations (3 levels)
fn benchmark_storage_deep_nested_map_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_deep_nested_map_insert");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut map = Root::new(|| {
                UnorderedMap::<String, UnorderedMap<String, UnorderedMap<String, String>>>::new()
            });

            b.iter(|| {
                for i in 0..size {
                    let key1 = format!("key1_{}", i);
                    let key2 = format!("key2_{}", i);
                    let key3 = format!("key3_{}", i);
                    let value = format!("value_{}", i);

                    // Get or create level 2 map
                    let mut level2_map = map
                        .entry(key1)
                        .unwrap()
                        .or_insert_with(|| UnorderedMap::new())
                        .unwrap();

                    // Get or create level 3 map
                    let mut level3_map = level2_map
                        .entry(key2)
                        .unwrap()
                        .or_insert_with(|| UnorderedMap::new())
                        .unwrap();

                    black_box(level3_map.insert(key3, value).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark Vector push operations
fn benchmark_storage_vector_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_push");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut vector = Root::new(|| Vector::new());

            b.iter(|| {
                for i in 0..size {
                    let value = format!("value_{}", i);
                    black_box(vector.push(value).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark Vector get operations
fn benchmark_storage_vector_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_get");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Push data first
            let mut vector = Root::new(|| Vector::new());
            for i in 0..100 {
                let value = format!("value_{}", i);
                vector.push(value).unwrap();
            }

            b.iter(|| {
                for i in 0..size {
                    black_box(vector.get(i % 100).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark Vector pop operations
fn benchmark_storage_vector_pop(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_pop");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Push data first
            let mut vector = Root::new(|| Vector::new());
            for i in 0..size {
                let value = format!("value_{}", i);
                vector.push(value).unwrap();
            }

            b.iter(|| {
                // Re-push after each iteration
                for i in 0..size {
                    let value = format!("value_{}", i);
                    vector.push(value).unwrap();
                    black_box(vector.pop().unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedSet insert operations
fn benchmark_storage_set_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_set_insert");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut set = Root::new(|| UnorderedSet::new());

            b.iter(|| {
                for i in 0..size {
                    let value = format!("value_{}", i);
                    black_box(set.insert(value).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedSet contains operations
fn benchmark_storage_set_contains(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_set_contains");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Insert data first
            let mut set = Root::new(|| UnorderedSet::new());
            for i in 0..100 {
                let value = format!("value_{}", i);
                set.insert(value).unwrap();
            }

            b.iter(|| {
                for i in 0..size {
                    let value = format!("value_{}", i % 100);
                    black_box(set.contains(&value).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark Counter increment operations
fn benchmark_storage_counter_increment(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_counter_increment");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut counter = Root::new(|| GCounter::new());

            b.iter(|| {
                for _i in 0..size {
                    black_box(counter.increment().unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark Counter get value operations
fn benchmark_storage_counter_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_counter_get");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Increment counter first
            let mut counter = Root::new(|| GCounter::new());
            for _i in 0..100 {
                counter.increment().unwrap();
            }

            b.iter(|| {
                for _i in 0..size {
                    black_box(counter.value_unsigned().unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark LwwRegister set operations
fn benchmark_storage_register_set(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_register_set");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut register = Root::new(|| LwwRegister::new(String::new()));

            b.iter(|| {
                for i in 0..size {
                    let value = format!("value_{}", i);
                    black_box(register.set(value));
                }
            });
        });
    }
    group.finish();
}

/// Benchmark LwwRegister get operations
fn benchmark_storage_register_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_register_get");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Set value first
            let mut register = Root::new(|| LwwRegister::new("initial".to_string()));
            register.set("test_value".to_string());

            b.iter(|| {
                for _i in 0..size {
                    black_box(register.get());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark RGA insert operations
fn benchmark_storage_rga_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_rga_insert");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut rga = Root::new(|| ReplicatedGrowableArray::new());

            b.iter(|| {
                for i in 0..size {
                    let text = format!("text_{}", i);
                    black_box(rga.insert_str(0, &text).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark RGA get_text operations
fn benchmark_storage_rga_get_text(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_rga_get_text");
    group.sample_size(10);

    for size in [10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Insert text first
            let mut rga = Root::new(|| ReplicatedGrowableArray::new());
            for i in 0..100 {
                let text = format!("text_{}", i);
                rga.insert_str(0, &text).unwrap();
            }

            b.iter(|| {
                for _i in 0..size {
                    black_box(rga.get_text().unwrap());
                }
            });
        });
    }
    group.finish();
}

// Multi-Threaded Benchmarks

/// Benchmark concurrent map inserts from multiple threads
fn benchmark_storage_map_insert_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_insert_concurrent");
    group.sample_size(10);

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for t in 0..thread_count {
                        let handle = std::thread::spawn(move || {
                            let mut map = Root::new(|| UnorderedMap::new());

                            // Each thread inserts 100 items
                            for i in 0..100 {
                                let key = format!("key_t{}_i{}", t, i);
                                let value = format!("value_t{}_i{}", t, i);
                                black_box(map.insert(key, value).unwrap());
                            }
                        });
                        handles.push(handle);
                    }

                    for handle in handles {
                        handle.join().unwrap();
                    }
                });
            },
        );
    }
    group.finish();
}

/// Benchmark concurrent vector pushes from multiple threads
fn benchmark_storage_vector_push_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_push_concurrent");
    group.sample_size(10);

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for _t in 0..thread_count {
                        let handle = std::thread::spawn(move || {
                            let mut vector = Root::new(|| Vector::new());

                            // Each thread pushes 100 items
                            for i in 0..100 {
                                let value = format!("value_{}", i);
                                black_box(vector.push(value).unwrap());
                            }
                        });
                        handles.push(handle);
                    }

                    for handle in handles {
                        handle.join().unwrap();
                    }
                });
            },
        );
    }
    group.finish();
}

/// Benchmark concurrent set inserts from multiple threads
fn benchmark_storage_set_insert_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_set_insert_concurrent");
    group.sample_size(10);

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for t in 0..thread_count {
                        let handle = std::thread::spawn(move || {
                            let mut set = Root::new(|| UnorderedSet::new());

                            // Each thread inserts 100 items
                            for i in 0..100 {
                                let value = format!("value_t{}_i{}", t, i);
                                black_box(set.insert(value).unwrap());
                            }
                        });
                        handles.push(handle);
                    }

                    for handle in handles {
                        handle.join().unwrap();
                    }
                });
            },
        );
    }
    group.finish();
}


// Criterion Configuration


/// Create a configured Criterion instance with enhanced visualization
fn configure_criterion() -> Criterion {
    Criterion::default()
        // Output directory for reports
        // Cargo workspace uses workspace root's target/ directory even when
        // running from a crate subdirectory, so this should resolve correctly
        .output_directory(Path::new("target/criterion/storage-benchmarks"))
        // Generate HTML reports with interactive plots
        // (enabled by default with html_reports feature)
        // Generate CSV files for data export
        .measurement_time(Duration::from_secs(10)) // Longer measurement time for better accuracy
        .warm_up_time(Duration::from_secs(3)) // Warm-up time
        .sample_size(10) // Minimum samples (can be overridden per group)
}


// Criterion Groups

criterion_group! {
    name = single_threaded;
    config = configure_criterion();
    targets =
        benchmark_storage_map_insert,
        benchmark_storage_map_get,
        benchmark_storage_map_remove,
        benchmark_storage_nested_map_insert,
        benchmark_storage_deep_nested_map_insert,
        benchmark_storage_vector_push,
        benchmark_storage_vector_get,
        benchmark_storage_vector_pop,
        benchmark_storage_set_insert,
        benchmark_storage_set_contains,
        benchmark_storage_counter_increment,
        benchmark_storage_counter_get,
        benchmark_storage_register_set,
        benchmark_storage_register_get,
        benchmark_storage_rga_insert,
        benchmark_storage_rga_get_text
}

criterion_group! {
    name = multi_threaded;
    config = configure_criterion();
    targets =
        benchmark_storage_map_insert_concurrent,
        benchmark_storage_vector_push_concurrent,
        benchmark_storage_set_insert_concurrent
}

criterion_main!(single_threaded, multi_threaded);
