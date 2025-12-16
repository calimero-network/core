//! Criterion benchmarks for storage collection operations
//!
//! These benchmarks measure pure CRDT collection performance without WASM or runtime overhead.

use calimero_storage::collections::{
    crdt_meta::Mergeable, GCounter, LwwRegister, ReplicatedGrowableArray, Root, UnorderedMap,
    UnorderedSet, Vector,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::path::Path;
use std::time::Duration;

// Size ranges for benchmarks
// Reduced from [10, 100, 1_000, 10_000, 100_000] to avoid CI timeouts
const STORAGE_BENCHMARK_SIZES: &[usize] = &[10, 100, 1_000];

// Value size variations for benchmarks (in bytes)
const VALUE_SIZE_SMALL: usize = 10; // ~10 bytes
const VALUE_SIZE_MEDIUM: usize = 100; // 100 bytes
const VALUE_SIZE_LARGE: usize = 1_000; // 1KB
const VALUE_SIZE_VERY_LARGE: usize = 10_000; // 10KB

// Helper function to create value of specified size
fn create_value_of_size(size: usize) -> String {
    "x".repeat(size)
}

// Single-Threaded Benchmarks

/// Benchmark UnorderedMap insert operations
fn benchmark_storage_map_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_insert");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut map = Root::new(|| UnorderedMap::new());
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut map =
                    Root::new(|| UnorderedMap::<String, UnorderedMap<String, String>>::new());
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut map = Root::new(|| {
                    UnorderedMap::<String, UnorderedMap<String, UnorderedMap<String, String>>>::new(
                    )
                });
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut vector = Root::new(|| Vector::new());
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut set = Root::new(|| UnorderedSet::new());
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut counter = Root::new(|| GCounter::new());
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut register = Root::new(|| LwwRegister::new(String::new()));
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut rga = Root::new(|| ReplicatedGrowableArray::new());
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

    for size in STORAGE_BENCHMARK_SIZES.iter() {
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

/// Benchmark UnorderedMap contains operations
fn benchmark_storage_map_contains(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_contains");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
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
                    black_box(map.contains(&key).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedMap merge operations
/// Note: Uses LwwRegister<String> as values since String doesn't implement Mergeable
fn benchmark_storage_map_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_merge");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                // Create two maps with LwwRegister values (which implement Mergeable)
                let mut map1 = Root::new(|| UnorderedMap::<String, LwwRegister<String>>::new());
                let mut map2 = Root::new(|| UnorderedMap::<String, LwwRegister<String>>::new());

                for i in 0..size {
                    let key = format!("key1_{}", i);
                    let value = LwwRegister::new(format!("value1_{}", i));
                    map1.insert(key, value).unwrap();
                }

                for i in 0..size {
                    let key = format!("key2_{}", i);
                    let value = LwwRegister::new(format!("value2_{}", i));
                    map2.insert(key, value).unwrap();
                }

                // Merge map2 into map1
                black_box(map1.merge(&map2).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedMap serialization
fn benchmark_storage_map_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_serialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create map with data
            let mut map = Root::new(|| UnorderedMap::new());
            for i in 0..size {
                let key = format!("key_{}", i);
                let value = format!("value_{}", i);
                map.insert(key, value).unwrap();
            }

            b.iter(|| {
                let mut buffer = Vec::new();
                black_box(borsh::to_writer(&mut buffer, &*map).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedMap deserialization
fn benchmark_storage_map_deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_deserialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create and serialize map
            let mut map = Root::new(|| UnorderedMap::new());
            for i in 0..size {
                let key = format!("key_{}", i);
                let value = format!("value_{}", i);
                map.insert(key, value).unwrap();
            }
            let serialized = borsh::to_vec(&*map).unwrap();

            b.iter(|| {
                black_box(borsh::from_slice::<UnorderedMap<String, String>>(&serialized).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedMap iteration
fn benchmark_storage_map_iter(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_iter");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Insert data first
            let mut map = Root::new(|| UnorderedMap::new());
            for i in 0..size {
                let key = format!("key_{}", i);
                let value = format!("value_{}", i);
                map.insert(key, value).unwrap();
            }

            b.iter(|| {
                let entries: Vec<_> = map.entries().unwrap().collect();
                black_box(entries.len());
            });
        });
    }
    group.finish();
}

/// Benchmark Vector merge operations
/// Note: Vector elements must implement Mergeable, so we use LwwRegister<String>
fn benchmark_storage_vector_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_merge");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                // Create two vectors with LwwRegister values (which implement Mergeable)
                let mut vector1 = Root::new(|| Vector::<LwwRegister<String>>::new());
                let mut vector2 = Root::new(|| Vector::<LwwRegister<String>>::new());

                for i in 0..size {
                    let value = LwwRegister::new(format!("value1_{}", i));
                    vector1.push(value).unwrap();
                }

                for i in 0..size {
                    let value = LwwRegister::new(format!("value2_{}", i));
                    vector2.push(value).unwrap();
                }

                // Merge vector2 into vector1
                black_box(vector1.merge(&vector2).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark Vector serialization
fn benchmark_storage_vector_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_serialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create vector with data
            let mut vector = Root::new(|| Vector::new());
            for i in 0..size {
                let value = format!("value_{}", i);
                vector.push(value).unwrap();
            }

            b.iter(|| {
                let mut buffer = Vec::new();
                black_box(borsh::to_writer(&mut buffer, &*vector).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark Vector deserialization
fn benchmark_storage_vector_deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_deserialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create and serialize vector
            let mut vector = Root::new(|| Vector::new());
            for i in 0..size {
                let value = format!("value_{}", i);
                vector.push(value).unwrap();
            }
            let serialized = borsh::to_vec(&*vector).unwrap();

            b.iter(|| {
                black_box(borsh::from_slice::<Vector<String>>(&serialized).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark Vector iteration
fn benchmark_storage_vector_iter(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_iter");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Insert data first
            let mut vector = Root::new(|| Vector::new());
            for i in 0..size {
                let value = format!("value_{}", i);
                vector.push(value).unwrap();
            }

            b.iter(|| {
                let items: Vec<_> = vector.iter().unwrap().collect();
                black_box(items.len());
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedSet merge operations
fn benchmark_storage_set_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_set_merge");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                // Create two sets with different data
                let mut set1 = Root::new(|| UnorderedSet::new());
                let mut set2 = Root::new(|| UnorderedSet::new());

                for i in 0..size {
                    let value = format!("value1_{}", i);
                    set1.insert(value).unwrap();
                }

                for i in 0..size {
                    let value = format!("value2_{}", i);
                    set2.insert(value).unwrap();
                }

                // Merge set2 into set1
                black_box(set1.merge(&set2).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedSet serialization
fn benchmark_storage_set_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_set_serialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create set with data
            let mut set = Root::new(|| UnorderedSet::new());
            for i in 0..size {
                let value = format!("value_{}", i);
                set.insert(value).unwrap();
            }

            b.iter(|| {
                let mut buffer = Vec::new();
                black_box(borsh::to_writer(&mut buffer, &*set).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedSet deserialization
fn benchmark_storage_set_deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_set_deserialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create and serialize set
            let mut set = Root::new(|| UnorderedSet::new());
            for i in 0..size {
                let value = format!("value_{}", i);
                set.insert(value).unwrap();
            }
            let serialized = borsh::to_vec(&*set).unwrap();

            b.iter(|| {
                black_box(borsh::from_slice::<UnorderedSet<String>>(&serialized).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark UnorderedSet iteration
fn benchmark_storage_set_iter(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_set_iter");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Insert data first
            let mut set = Root::new(|| UnorderedSet::new());
            for i in 0..size {
                let value = format!("value_{}", i);
                set.insert(value).unwrap();
            }

            b.iter(|| {
                let items: Vec<_> = set.iter().unwrap().collect();
                black_box(items.len());
            });
        });
    }
    group.finish();
}

/// Benchmark Counter merge operations
fn benchmark_storage_counter_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_counter_merge");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                // Create two counters
                let mut counter1 = Root::new(|| GCounter::new());
                let mut counter2 = Root::new(|| GCounter::new());

                for _i in 0..size {
                    counter1.increment().unwrap();
                }

                for _i in 0..size {
                    counter2.increment().unwrap();
                }

                // Merge counter2 into counter1
                black_box(counter1.merge(&counter2).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark Counter serialization
fn benchmark_storage_counter_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_counter_serialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create counter with increments
            let mut counter = Root::new(|| GCounter::new());
            for _i in 0..size {
                counter.increment().unwrap();
            }

            b.iter(|| {
                let mut buffer = Vec::new();
                black_box(borsh::to_writer(&mut buffer, &*counter).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark Counter deserialization
fn benchmark_storage_counter_deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_counter_deserialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create and serialize counter
            let mut counter = Root::new(|| GCounter::new());
            for _i in 0..size {
                counter.increment().unwrap();
            }
            let serialized = borsh::to_vec(&*counter).unwrap();

            b.iter(|| {
                black_box(borsh::from_slice::<GCounter>(&serialized).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark LwwRegister merge operations
fn benchmark_storage_register_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_register_merge");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                // Create two registers
                let mut register1 = Root::new(|| LwwRegister::new("value1".to_string()));
                let mut register2 = Root::new(|| LwwRegister::new("value2".to_string()));

                for i in 0..size {
                    register1.set(format!("value1_{}", i));
                    register2.set(format!("value2_{}", i));
                }

                // Merge register2 into register1
                register1.merge(&register2);
                black_box(register1.get());
            });
        });
    }
    group.finish();
}

/// Benchmark LwwRegister serialization
fn benchmark_storage_register_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_register_serialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create register with value
            let mut register = Root::new(|| LwwRegister::new("initial".to_string()));
            for i in 0..size {
                register.set(format!("value_{}", i));
            }

            b.iter(|| {
                let mut buffer = Vec::new();
                black_box(borsh::to_writer(&mut buffer, &*register).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark LwwRegister deserialization
fn benchmark_storage_register_deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_register_deserialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create and serialize register
            let mut register = Root::new(|| LwwRegister::new("initial".to_string()));
            for i in 0..size {
                register.set(format!("value_{}", i));
            }
            let serialized = borsh::to_vec(&*register).unwrap();

            b.iter(|| {
                black_box(borsh::from_slice::<LwwRegister<String>>(&serialized).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark RGA merge operations
fn benchmark_storage_rga_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_rga_merge");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                // Create two RGAs
                let mut rga1 = Root::new(|| ReplicatedGrowableArray::new());
                let mut rga2 = Root::new(|| ReplicatedGrowableArray::new());

                for i in 0..size {
                    let text = format!("text1_{}", i);
                    rga1.insert_str(0, &text).unwrap();
                }

                for i in 0..size {
                    let text = format!("text2_{}", i);
                    rga2.insert_str(0, &text).unwrap();
                }

                // Merge rga2 into rga1
                black_box(rga1.merge(&rga2).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark RGA delete operations
fn benchmark_storage_rga_delete(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_rga_delete");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Insert data first
            let mut rga = Root::new(|| ReplicatedGrowableArray::new());
            for i in 0..size {
                let text = format!("text_{}", i);
                rga.insert_str(0, &text).unwrap();
            }

            b.iter(|| {
                // Track length before inserting
                let initial_len = rga.len().unwrap();

                // Insert strings (each string adds multiple characters)
                let mut total_chars_inserted = 0;
                for i in 0..size {
                    let text = format!("text_{}", i);
                    let text_len = text.len();
                    rga.insert_str(0, &text).unwrap();
                    total_chars_inserted += text_len;
                }

                // Delete all characters we just inserted (from end to maintain indices)
                let final_len = rga.len().unwrap();
                for i in (initial_len..final_len).rev() {
                    black_box(rga.delete(i).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark RGA delete_range operations
fn benchmark_storage_rga_delete_range(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_rga_delete_range");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Insert data first
            let mut rga = Root::new(|| ReplicatedGrowableArray::new());
            for i in 0..size {
                let text = format!("text_{}", i);
                rga.insert_str(0, &text).unwrap();
            }

            b.iter(|| {
                // Track length before inserting
                let initial_len = rga.len().unwrap();

                // Insert strings (each string adds multiple characters)
                for i in 0..size {
                    let text = format!("text_{}", i);
                    rga.insert_str(0, &text).unwrap();
                }

                // Delete all characters we just inserted using delete_range
                // Since we insert at position 0, new content is at positions 0 to (final_len - initial_len - 1)
                let final_len = rga.len().unwrap();
                if final_len > initial_len {
                    let chars_inserted = final_len - initial_len;
                    // Delete the range containing all newly inserted characters
                    black_box(rga.delete_range(0, chars_inserted).unwrap());
                }
            });
        });
    }
    group.finish();
}

/// Benchmark RGA serialization
fn benchmark_storage_rga_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_rga_serialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create RGA with data
            let mut rga = Root::new(|| ReplicatedGrowableArray::new());
            for i in 0..size {
                let text = format!("text_{}", i);
                rga.insert_str(0, &text).unwrap();
            }

            b.iter(|| {
                let mut buffer = Vec::new();
                black_box(borsh::to_writer(&mut buffer, &*rga).unwrap());
            });
        });
    }
    group.finish();
}

/// Benchmark RGA deserialization
fn benchmark_storage_rga_deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_rga_deserialize");
    group.sample_size(10);

    for size in STORAGE_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Setup: Create and serialize RGA
            let mut rga = Root::new(|| ReplicatedGrowableArray::new());
            for i in 0..size {
                let text = format!("text_{}", i);
                rga.insert_str(0, &text).unwrap();
            }
            let serialized = borsh::to_vec(&*rga).unwrap();

            b.iter(|| {
                black_box(borsh::from_slice::<ReplicatedGrowableArray>(&serialized).unwrap());
            });
        });
    }
    group.finish();
}

// Value Size Variation Benchmarks

/// Benchmark UnorderedMap insert with different value sizes
fn benchmark_storage_map_insert_value_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_insert_value_sizes");
    group.sample_size(10);

    let value_sizes = [
        ("small", VALUE_SIZE_SMALL),
        ("medium", VALUE_SIZE_MEDIUM),
        ("large", VALUE_SIZE_LARGE),
        ("very_large", VALUE_SIZE_VERY_LARGE),
    ];

    for (size_name, value_size) in value_sizes.iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(size_name),
            value_size,
            |b, &value_size| {
                let mut map = Root::new(|| UnorderedMap::new());
                let value_template = create_value_of_size(value_size);

                b.iter(|| {
                    for i in 0..100 {
                        let key = format!("key_{}", i);
                        let value = format!("{}_{}", value_template, i);
                        black_box(map.insert(key, value).unwrap());
                    }
                });
            },
        );
    }
    group.finish();
}

/// Benchmark Vector push with different value sizes
fn benchmark_storage_vector_push_value_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_push_value_sizes");
    group.sample_size(10);

    let value_sizes = [
        ("small", VALUE_SIZE_SMALL),
        ("medium", VALUE_SIZE_MEDIUM),
        ("large", VALUE_SIZE_LARGE),
        ("very_large", VALUE_SIZE_VERY_LARGE),
    ];

    for (size_name, value_size) in value_sizes.iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(size_name),
            value_size,
            |b, &value_size| {
                let mut vector = Root::new(|| Vector::new());
                let value_template = create_value_of_size(value_size);

                b.iter(|| {
                    for i in 0..100 {
                        let value = format!("{}_{}", value_template, i);
                        black_box(vector.push(value).unwrap());
                    }
                });
            },
        );
    }
    group.finish();
}

/// Benchmark UnorderedSet insert with different value sizes
fn benchmark_storage_set_insert_value_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_set_insert_value_sizes");
    group.sample_size(10);

    let value_sizes = [
        ("small", VALUE_SIZE_SMALL),
        ("medium", VALUE_SIZE_MEDIUM),
        ("large", VALUE_SIZE_LARGE),
        ("very_large", VALUE_SIZE_VERY_LARGE),
    ];

    for (size_name, value_size) in value_sizes.iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(size_name),
            value_size,
            |b, &value_size| {
                let mut set = Root::new(|| UnorderedSet::new());
                let value_template = create_value_of_size(value_size);

                b.iter(|| {
                    for i in 0..100 {
                        let value = format!("{}_{}", value_template, i);
                        black_box(set.insert(value).unwrap());
                    }
                });
            },
        );
    }
    group.finish();
}

// Edge Case Benchmarks

/// Benchmark UnorderedMap operations on empty collection
fn benchmark_storage_map_empty_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_empty_operations");
    group.sample_size(10);

    group.bench_function("first_insert", |b| {
        let mut map = Root::new(|| UnorderedMap::<String, String>::new());
        b.iter(|| {
            black_box(map.insert("key".to_string(), "value".to_string()).unwrap());
        });
    });

    group.bench_function("get_from_empty", |b| {
        let map = Root::new(|| UnorderedMap::<String, String>::new());
        b.iter(|| {
            black_box(map.get("key").unwrap());
        });
    });

    group.bench_function("contains_on_empty", |b| {
        let map = Root::new(|| UnorderedMap::<String, String>::new());
        b.iter(|| {
            black_box(map.contains("key").unwrap());
        });
    });

    group.finish();
}

/// Benchmark Vector operations on empty collection
fn benchmark_storage_vector_empty_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_empty_operations");
    group.sample_size(10);

    group.bench_function("first_push", |b| {
        let mut vector = Root::new(|| Vector::new());
        b.iter(|| {
            black_box(vector.push("value".to_string()).unwrap());
        });
    });

    group.bench_function("get_from_empty", |b| {
        let vector = Root::new(|| Vector::<String>::new());
        b.iter(|| {
            black_box(vector.get(0).unwrap());
        });
    });

    group.bench_function("pop_from_empty", |b| {
        let mut vector = Root::new(|| Vector::<String>::new());
        b.iter(|| {
            black_box(vector.pop().unwrap());
        });
    });

    group.finish();
}

/// Benchmark UnorderedMap operations with single element
fn benchmark_storage_map_single_element(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_single_element");
    group.sample_size(10);

    group.bench_function("insert", |b| {
        let mut map = Root::new(|| UnorderedMap::<String, String>::new());
        map.insert("key1".to_string(), "value1".to_string())
            .unwrap();
        b.iter(|| {
            black_box(
                map.insert("key2".to_string(), "value2".to_string())
                    .unwrap(),
            );
        });
    });

    group.bench_function("get", |b| {
        let mut map = Root::new(|| UnorderedMap::<String, String>::new());
        map.insert("key".to_string(), "value".to_string()).unwrap();
        b.iter(|| {
            black_box(map.get("key").unwrap());
        });
    });

    group.bench_function("remove", |b| {
        let mut map = Root::new(|| UnorderedMap::<String, String>::new());
        map.insert("key".to_string(), "value".to_string()).unwrap();
        b.iter(|| {
            map.insert("key".to_string(), "value".to_string()).unwrap();
            black_box(map.remove("key").unwrap());
        });
    });

    group.finish();
}

/// Benchmark Vector operations with single element
fn benchmark_storage_vector_single_element(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_single_element");
    group.sample_size(10);

    group.bench_function("push", |b| {
        let mut vector = Root::new(|| Vector::<String>::new());
        vector.push("value1".to_string()).unwrap();
        b.iter(|| {
            black_box(vector.push("value2".to_string()).unwrap());
        });
    });

    group.bench_function("get", |b| {
        let mut vector = Root::new(|| Vector::<String>::new());
        vector.push("value".to_string()).unwrap();
        b.iter(|| {
            black_box(vector.get(0).unwrap());
        });
    });

    group.bench_function("pop", |b| {
        let mut vector = Root::new(|| Vector::<String>::new());
        vector.push("value".to_string()).unwrap();
        b.iter(|| {
            vector.push("value".to_string()).unwrap();
            black_box(vector.pop().unwrap());
        });
    });

    group.finish();
}

// Memory/Space Benchmarks

/// Benchmark UnorderedMap memory usage per element
fn benchmark_storage_map_memory_per_element(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_map_memory_per_element");
    group.sample_size(10);

    for size in [10, 100, 1_000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut map = Root::new(|| UnorderedMap::new());
            for i in 0..size {
                let key = format!("key_{}", i);
                let value = format!("value_{}", i);
                map.insert(key, value).unwrap();
            }

            // Serialize to measure size
            let serialized = borsh::to_vec(&*map).unwrap();
            let serialized_size = serialized.len();

            b.iter(|| {
                // Measure serialized size
                let serialized = borsh::to_vec(&*map).unwrap();
                black_box(serialized.len());
            });

            // Report bytes per element
            let bytes_per_element = if size > 0 {
                serialized_size as f64 / size as f64
            } else {
                0.0
            };
            eprintln!(
                "Map with {} elements: {} bytes total, {:.2} bytes/element",
                size, serialized_size, bytes_per_element
            );
        });
    }
    group.finish();
}

/// Benchmark Vector memory usage per element
fn benchmark_storage_vector_memory_per_element(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_vector_memory_per_element");
    group.sample_size(10);

    for size in [10, 100, 1_000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut vector = Root::new(|| Vector::new());
            for i in 0..size {
                let value = format!("value_{}", i);
                vector.push(value).unwrap();
            }

            // Serialize to measure size
            let serialized = borsh::to_vec(&*vector).unwrap();
            let serialized_size = serialized.len();

            b.iter(|| {
                // Measure serialized size
                let serialized = borsh::to_vec(&*vector).unwrap();
                black_box(serialized.len());
            });

            // Report bytes per element
            let bytes_per_element = if size > 0 {
                serialized_size as f64 / size as f64
            } else {
                0.0
            };
            eprintln!(
                "Vector with {} elements: {} bytes total, {:.2} bytes/element",
                size, serialized_size, bytes_per_element
            );
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

/// Benchmark concurrent counter increments from multiple threads
fn benchmark_storage_counter_increment_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_counter_increment_concurrent");
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
                            let mut counter = Root::new(|| GCounter::new());

                            // Each thread increments 100 times
                            for _i in 0..100 {
                                black_box(counter.increment().unwrap());
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

/// Benchmark concurrent register sets from multiple threads
fn benchmark_storage_register_set_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_register_set_concurrent");
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
                            let mut register =
                                Root::new(|| LwwRegister::new("initial".to_string()));

                            // Each thread sets register 100 times
                            for i in 0..100 {
                                let value = format!("value_t{}_i{}", t, i);
                                register.set(value);
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

/// Benchmark concurrent RGA inserts from multiple threads
fn benchmark_storage_rga_insert_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_rga_insert_concurrent");
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
                            let mut rga = Root::new(|| ReplicatedGrowableArray::new());

                            // Each thread inserts 100 characters
                            for i in 0..100 {
                                let text = format!("t{}_i{}", t, i);
                                black_box(rga.insert_str(0, &text).unwrap());
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
        .sample_size(20) // Increased samples for better statistical confidence
}

// Criterion Groups

criterion_group! {
    name = single_threaded;
    config = configure_criterion();
    targets =
        benchmark_storage_map_insert,
        benchmark_storage_map_get,
        benchmark_storage_map_remove,
        benchmark_storage_map_contains,
        benchmark_storage_map_merge,
        benchmark_storage_map_serialize,
        benchmark_storage_map_deserialize,
        benchmark_storage_map_iter,
        benchmark_storage_nested_map_insert,
        benchmark_storage_deep_nested_map_insert,
        benchmark_storage_vector_push,
        benchmark_storage_vector_get,
        benchmark_storage_vector_pop,
        benchmark_storage_vector_merge,
        benchmark_storage_vector_serialize,
        benchmark_storage_vector_deserialize,
        benchmark_storage_vector_iter,
        benchmark_storage_set_insert,
        benchmark_storage_set_contains,
        benchmark_storage_set_merge,
        benchmark_storage_set_serialize,
        benchmark_storage_set_deserialize,
        benchmark_storage_set_iter,
        benchmark_storage_counter_increment,
        benchmark_storage_counter_get,
        benchmark_storage_counter_merge,
        benchmark_storage_counter_serialize,
        benchmark_storage_counter_deserialize,
        benchmark_storage_register_set,
        benchmark_storage_register_get,
        benchmark_storage_register_merge,
        benchmark_storage_register_serialize,
        benchmark_storage_register_deserialize,
        benchmark_storage_rga_insert,
        benchmark_storage_rga_get_text,
        benchmark_storage_rga_delete,
        benchmark_storage_rga_delete_range,
        benchmark_storage_rga_merge,
        benchmark_storage_rga_serialize,
        benchmark_storage_rga_deserialize
}

criterion_group! {
    name = value_sizes;
    config = configure_criterion();
    targets =
        benchmark_storage_map_insert_value_sizes,
        benchmark_storage_vector_push_value_sizes,
        benchmark_storage_set_insert_value_sizes
}

criterion_group! {
    name = edge_cases;
    config = configure_criterion();
    targets =
        benchmark_storage_map_empty_operations,
        benchmark_storage_vector_empty_operations,
        benchmark_storage_map_single_element,
        benchmark_storage_vector_single_element
}

criterion_group! {
    name = memory;
    config = configure_criterion();
    targets =
        benchmark_storage_map_memory_per_element,
        benchmark_storage_vector_memory_per_element
}

criterion_group! {
    name = multi_threaded;
    config = configure_criterion();
    targets =
        benchmark_storage_map_insert_concurrent,
        benchmark_storage_vector_push_concurrent,
        benchmark_storage_set_insert_concurrent,
        benchmark_storage_counter_increment_concurrent,
        benchmark_storage_register_set_concurrent,
        benchmark_storage_rga_insert_concurrent
}

criterion_main!(
    single_threaded,
    multi_threaded,
    value_sizes,
    edge_cases,
    memory
);
