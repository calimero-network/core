//! Criterion benchmarks for runtime collection operations
//!
//! These benchmarks measure WASM execution performance through the runtime,
//! including host function overhead and CRDT operations.

use calimero_runtime::store::InMemoryStorage;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::path::Path;
use std::time::Duration;

mod common;

use common::*;

// Size ranges for benchmarks
// Reduced from [10, 100, 1_000] to avoid CI timeouts
const RUNTIME_BENCHMARK_SIZES: &[usize] = &[10, 100];

// Single-Threaded Benchmarks

/// Benchmark map insert operations
fn benchmark_runtime_map_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_map_insert");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            b.iter(|| {
                for i in 0..size {
                    let key = format!("key_{}", i);
                    let input = json_input("key", &key);
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "map_insert",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark map get operations
fn benchmark_runtime_map_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_map_get");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    // First, insert some data
    let mut storage = InMemoryStorage::default();
    init_app(&module, &mut storage);

    // Insert 100 items
    for i in 0..100 {
        let key = format!("key_{}", i);
        let input = json_input("key", &key);
        module
            .run(
                context_id,
                executor,
                "map_insert",
                &input,
                &mut storage,
                None,
                None,
            )
            .unwrap();
    }

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Create a fresh storage for each iteration
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            // Insert items for reading
            for i in 0..100 {
                let key = format!("key_{}", i);
                let input = json_input("key", &key);
                module
                    .run(
                        context_id,
                        executor,
                        "map_insert",
                        &input,
                        &mut storage,
                        None,
                        None,
                    )
                    .unwrap();
            }

            b.iter(|| {
                for i in 0..size {
                    let key = format!("key_{}", i % 100);
                    let input = json_input("key", &key);
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "map_get",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark nested map insert operations
fn benchmark_runtime_nested_map_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_nested_map_insert");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            b.iter(|| {
                for i in 0..size {
                    let outer_key = format!("outer_{}", i);
                    let inner_key = format!("inner_{}", i);
                    let input =
                        json_input_multi(&[("outer_key", &outer_key), ("inner_key", &inner_key)]);
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "nested_map_insert",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark vector push operations
fn benchmark_runtime_vector_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_vector_push");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            b.iter(|| {
                for _i in 0..size {
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "vector_push",
                                &[],
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark vector get operations
fn benchmark_runtime_vector_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_vector_get");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    // First, push some data
    let mut storage = InMemoryStorage::default();
    init_app(&module, &mut storage);

    // Push 100 items
    for _i in 0..100 {
        module
            .run(
                context_id,
                executor,
                "vector_push",
                &[],
                &mut storage,
                None,
                None,
            )
            .unwrap();
    }

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Create a fresh storage for each iteration
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            // Push items for reading
            for _i in 0..100 {
                module
                    .run(
                        context_id,
                        executor,
                        "vector_push",
                        &[],
                        &mut storage,
                        None,
                        None,
                    )
                    .unwrap();
            }

            b.iter(|| {
                for i in 0..size {
                    let input = serde_json::to_vec(&serde_json::json!({"index": i % 100})).unwrap();
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "vector_get",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark set insert operations
fn benchmark_runtime_set_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_set_insert");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            b.iter(|| {
                for i in 0..size {
                    let value = format!("value_{}", i);
                    let input = json_input("value", &value);
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "set_insert",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark set contains operations
fn benchmark_runtime_set_contains(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_set_contains");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    // First, insert some data
    let mut storage = InMemoryStorage::default();
    init_app(&module, &mut storage);

    // Insert 100 items
    for i in 0..100 {
        let value = format!("value_{}", i);
        let input = json_input("value", &value);
        module
            .run(
                context_id,
                executor,
                "set_insert",
                &input,
                &mut storage,
                None,
                None,
            )
            .unwrap();
    }

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Create a fresh storage for each iteration
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            // Insert items for checking
            for i in 0..100 {
                let value = format!("value_{}", i);
                let input = json_input("value", &value);
                module
                    .run(
                        context_id,
                        executor,
                        "set_insert",
                        &input,
                        &mut storage,
                        None,
                        None,
                    )
                    .unwrap();
            }

            b.iter(|| {
                for i in 0..size {
                    let value = format!("value_{}", i % 100);
                    let input = json_input("value", &value);
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "set_contains",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark map remove operations
fn benchmark_runtime_map_remove(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_map_remove");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            b.iter(|| {
                // Insert items first
                for i in 0..size {
                    let key = format!("key_{}", i);
                    let input = json_input("key", &key);
                    module
                        .run(
                            context_id,
                            executor,
                            "map_insert",
                            &input,
                            &mut storage,
                            None,
                            None,
                        )
                        .unwrap();
                }

                // Then remove them (if map_remove exists in WASM)
                // Note: This assumes map_remove method exists in the WASM module
                for i in 0..size {
                    let key = format!("key_{}", i);
                    let input = json_input("key", &key);
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "map_remove",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark map contains operations
fn benchmark_runtime_map_contains(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_map_contains");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    // First, insert some data
    let mut storage = InMemoryStorage::default();
    init_app(&module, &mut storage);

    // Insert 100 items
    for i in 0..100 {
        let key = format!("key_{}", i);
        let input = json_input("key", &key);
        module
            .run(
                context_id,
                executor,
                "map_insert",
                &input,
                &mut storage,
                None,
                None,
            )
            .unwrap();
    }

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Create a fresh storage for each iteration
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            // Insert items for checking
            for i in 0..100 {
                let key = format!("key_{}", i);
                let input = json_input("key", &key);
                module
                    .run(
                        context_id,
                        executor,
                        "map_insert",
                        &input,
                        &mut storage,
                        None,
                        None,
                    )
                    .unwrap();
            }

            b.iter(|| {
                for i in 0..size {
                    let key = format!("key_{}", i % 100);
                    let input = json_input("key", &key);
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "map_contains",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark vector pop operations
fn benchmark_runtime_vector_pop(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_vector_pop");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            b.iter(|| {
                // Push items first
                for _i in 0..size {
                    module
                        .run(
                            context_id,
                            executor,
                            "vector_push",
                            &[],
                            &mut storage,
                            None,
                            None,
                        )
                        .unwrap();
                }

                // Then pop them (if vector_pop exists in WASM)
                for _i in 0..size {
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "vector_pop",
                                &[],
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark counter increment operations
fn benchmark_runtime_counter_increment(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_counter_increment");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            b.iter(|| {
                for _i in 0..size {
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "counter_increment",
                                &[],
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark counter get operations
fn benchmark_runtime_counter_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_counter_get");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    // First, increment counter
    let mut storage = InMemoryStorage::default();
    init_app(&module, &mut storage);

    // Increment 100 times
    for _i in 0..100 {
        module
            .run(
                context_id,
                executor,
                "counter_increment",
                &[],
                &mut storage,
                None,
                None,
            )
            .unwrap();
    }

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Create a fresh storage for each iteration
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            // Increment counter for reading
            for _i in 0..100 {
                module
                    .run(
                        context_id,
                        executor,
                        "counter_increment",
                        &[],
                        &mut storage,
                        None,
                        None,
                    )
                    .unwrap();
            }

            b.iter(|| {
                for _i in 0..size {
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "counter_get",
                                &[],
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark register get operations
fn benchmark_runtime_register_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_register_get");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    // First, set register
    let mut storage = InMemoryStorage::default();
    init_app(&module, &mut storage);

    // Set register value
    let input = json_input_multi(&[("key", "register_key"), ("value", "test_value")]);
    module
        .run(
            context_id,
            executor,
            "register_set",
            &input,
            &mut storage,
            None,
            None,
        )
        .unwrap();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Create a fresh storage for each iteration
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            // Set register value for reading
            let input = json_input_multi(&[("key", "register_key"), ("value", "test_value")]);
            module
                .run(
                    context_id,
                    executor,
                    "register_set",
                    &input,
                    &mut storage,
                    None,
                    None,
                )
                .unwrap();

            b.iter(|| {
                for _i in 0..size {
                    let input = json_input("key", "register_key");
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "register_get",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark RGA get_text operations
fn benchmark_runtime_rga_get_text(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_rga_get_text");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    // First, insert some text
    let mut storage = InMemoryStorage::default();
    init_app(&module, &mut storage);

    // Insert 100 characters
    for i in 0..100 {
        let text = format!("{}", i % 10);
        let input = json_input_multi(&[("index", "0"), ("text", &text)]);
        module
            .run(
                context_id,
                executor,
                "rga_insert",
                &input,
                &mut storage,
                None,
                None,
            )
            .unwrap();
    }

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Create a fresh storage for each iteration
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            // Insert text for reading
            for i in 0..100 {
                let text = format!("{}", i % 10);
                let input = json_input_multi(&[("index", "0"), ("text", &text)]);
                module
                    .run(
                        context_id,
                        executor,
                        "rga_insert",
                        &input,
                        &mut storage,
                        None,
                        None,
                    )
                    .unwrap();
            }

            b.iter(|| {
                for _i in 0..size {
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "rga_get_text",
                                &[],
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark register set operations
fn benchmark_runtime_register_set(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_register_set");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            b.iter(|| {
                for i in 0..size {
                    let key = format!("key_{}", i);
                    let value = format!("value_{}", i);
                    let input = json_input_multi(&[("key", &key), ("value", &value)]);
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "register_set",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark RGA insert operations
fn benchmark_runtime_rga_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_rga_insert");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            b.iter(|| {
                for i in 0..size {
                    let text = format!("text_{}", i);
                    let input = json_input_multi(&[("index", "0"), ("text", &text)]);
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "rga_insert",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

/// Benchmark deep nested map insert operations (3 levels)
fn benchmark_runtime_deep_nested_map_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_deep_nested_map_insert");
    group.sample_size(10);

    let module = compile_benchmark_module();
    let context_id = default_context_id();
    let executor = default_executor();

    for size in RUNTIME_BENCHMARK_SIZES.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let mut storage = InMemoryStorage::default();
            init_app(&module, &mut storage);

            b.iter(|| {
                for i in 0..size {
                    let key1 = format!("key1_{}", i);
                    let key2 = format!("key2_{}", i);
                    let key3 = format!("key3_{}", i);
                    let input =
                        json_input_multi(&[("key1", &key1), ("key2", &key2), ("key3", &key3)]);
                    black_box(
                        module
                            .run(
                                context_id,
                                executor,
                                "deep_nested_map_insert",
                                &input,
                                &mut storage,
                                None,
                                None,
                            )
                            .unwrap(),
                    );
                }
            });
        });
    }
    group.finish();
}

// Multi-Threaded Benchmarks

/// Benchmark concurrent map inserts from multiple threads
///
/// Note: Each thread compiles its own WASM module to avoid sharing issues.
/// This measures concurrent execution but with separate storage instances.
fn benchmark_runtime_map_insert_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_map_insert_concurrent");
    group.sample_size(10);

    let context_id = default_context_id();
    let executor = default_executor();

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for t in 0..thread_count {
                        let context_id = context_id;
                        let executor = executor;

                        let handle = std::thread::spawn(move || {
                            // Each thread compiles its own module
                            let module = compile_benchmark_module();
                            let mut storage = InMemoryStorage::default();
                            init_app(&module, &mut storage);

                            // Each thread inserts 100 items
                            for i in 0..100 {
                                let key = format!("key_t{}_i{}", t, i);
                                let input = json_input("key", &key);
                                black_box(
                                    module
                                        .run(
                                            context_id,
                                            executor,
                                            "map_insert",
                                            &input,
                                            &mut storage,
                                            None,
                                            None,
                                        )
                                        .unwrap(),
                                );
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
///
/// Note: Each thread compiles its own WASM module to avoid sharing issues.
/// This measures concurrent execution but with separate storage instances.
fn benchmark_runtime_vector_push_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_vector_push_concurrent");
    group.sample_size(10);

    let context_id = default_context_id();
    let executor = default_executor();

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for _t in 0..thread_count {
                        let context_id = context_id;
                        let executor = executor;

                        let handle = std::thread::spawn(move || {
                            // Each thread compiles its own module
                            let module = compile_benchmark_module();
                            let mut storage = InMemoryStorage::default();
                            init_app(&module, &mut storage);

                            // Each thread pushes 100 items
                            for _i in 0..100 {
                                black_box(
                                    module
                                        .run(
                                            context_id,
                                            executor,
                                            "vector_push",
                                            &[],
                                            &mut storage,
                                            None,
                                            None,
                                        )
                                        .unwrap(),
                                );
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

/// Benchmark concurrent nested map inserts from multiple threads
///
/// Note: Each thread compiles its own WASM module to avoid sharing issues.
/// This measures concurrent execution but with separate storage instances.
fn benchmark_runtime_nested_map_insert_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_nested_map_insert_concurrent");
    group.sample_size(10);

    let context_id = default_context_id();
    let executor = default_executor();

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for t in 0..thread_count {
                        let context_id = context_id;
                        let executor = executor;

                        let handle = std::thread::spawn(move || {
                            // Each thread compiles its own module
                            let module = compile_benchmark_module();
                            let mut storage = InMemoryStorage::default();
                            init_app(&module, &mut storage);

                            // Each thread inserts 100 nested items
                            for i in 0..100 {
                                let outer_key = format!("outer_t{}_i{}", t, i);
                                let inner_key = format!("inner_t{}_i{}", t, i);
                                let input = json_input_multi(&[
                                    ("outer_key", &outer_key),
                                    ("inner_key", &inner_key),
                                ]);
                                black_box(
                                    module
                                        .run(
                                            context_id,
                                            executor,
                                            "nested_map_insert",
                                            &input,
                                            &mut storage,
                                            None,
                                            None,
                                        )
                                        .unwrap(),
                                );
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

/// Benchmark concurrent deep nested map inserts from multiple threads
///
/// Note: Each thread compiles its own WASM module to avoid sharing issues.
/// This measures concurrent execution but with separate storage instances.
fn benchmark_runtime_deep_nested_map_insert_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_deep_nested_map_insert_concurrent");
    group.sample_size(10);

    let context_id = default_context_id();
    let executor = default_executor();

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for t in 0..thread_count {
                        let context_id = context_id;
                        let executor = executor;

                        let handle = std::thread::spawn(move || {
                            // Each thread compiles its own module
                            let module = compile_benchmark_module();
                            let mut storage = InMemoryStorage::default();
                            init_app(&module, &mut storage);

                            // Each thread inserts 100 deep nested items
                            for i in 0..100 {
                                let key1 = format!("key1_t{}_i{}", t, i);
                                let key2 = format!("key2_t{}_i{}", t, i);
                                let key3 = format!("key3_t{}_i{}", t, i);
                                let input = json_input_multi(&[
                                    ("key1", &key1),
                                    ("key2", &key2),
                                    ("key3", &key3),
                                ]);
                                black_box(
                                    module
                                        .run(
                                            context_id,
                                            executor,
                                            "deep_nested_map_insert",
                                            &input,
                                            &mut storage,
                                            None,
                                            None,
                                        )
                                        .unwrap(),
                                );
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
///
/// Note: Each thread compiles its own WASM module to avoid sharing issues.
/// This measures concurrent execution but with separate storage instances.
fn benchmark_runtime_set_insert_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_set_insert_concurrent");
    group.sample_size(10);

    let context_id = default_context_id();
    let executor = default_executor();

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for t in 0..thread_count {
                        let context_id = context_id;
                        let executor = executor;

                        let handle = std::thread::spawn(move || {
                            // Each thread compiles its own module
                            let module = compile_benchmark_module();
                            let mut storage = InMemoryStorage::default();
                            init_app(&module, &mut storage);

                            // Each thread inserts 100 items
                            for i in 0..100 {
                                let value = format!("value_t{}_i{}", t, i);
                                let input = json_input("value", &value);
                                black_box(
                                    module
                                        .run(
                                            context_id,
                                            executor,
                                            "set_insert",
                                            &input,
                                            &mut storage,
                                            None,
                                            None,
                                        )
                                        .unwrap(),
                                );
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
///
/// Note: Each thread compiles its own WASM module to avoid sharing issues.
/// This measures concurrent execution but with separate storage instances.
fn benchmark_runtime_counter_increment_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_counter_increment_concurrent");
    group.sample_size(10);

    let context_id = default_context_id();
    let executor = default_executor();

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for _t in 0..thread_count {
                        let context_id = context_id;
                        let executor = executor;

                        let handle = std::thread::spawn(move || {
                            // Each thread compiles its own module
                            let module = compile_benchmark_module();
                            let mut storage = InMemoryStorage::default();
                            init_app(&module, &mut storage);

                            // Each thread increments 100 times
                            for _i in 0..100 {
                                black_box(
                                    module
                                        .run(
                                            context_id,
                                            executor,
                                            "counter_increment",
                                            &[],
                                            &mut storage,
                                            None,
                                            None,
                                        )
                                        .unwrap(),
                                );
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
///
/// Note: Each thread compiles its own WASM module to avoid sharing issues.
/// This measures concurrent execution but with separate storage instances.
fn benchmark_runtime_register_set_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_register_set_concurrent");
    group.sample_size(10);

    let context_id = default_context_id();
    let executor = default_executor();

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for t in 0..thread_count {
                        let context_id = context_id;
                        let executor = executor;

                        let handle = std::thread::spawn(move || {
                            // Each thread compiles its own module
                            let module = compile_benchmark_module();
                            let mut storage = InMemoryStorage::default();
                            init_app(&module, &mut storage);

                            // Each thread sets register 100 times
                            for i in 0..100 {
                                let key = format!("key_t{}", t);
                                let value = format!("value_t{}_i{}", t, i);
                                let input = json_input_multi(&[("key", &key), ("value", &value)]);
                                black_box(
                                    module
                                        .run(
                                            context_id,
                                            executor,
                                            "register_set",
                                            &input,
                                            &mut storage,
                                            None,
                                            None,
                                        )
                                        .unwrap(),
                                );
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
///
/// Note: Each thread compiles its own WASM module to avoid sharing issues.
/// This measures concurrent execution but with separate storage instances.
fn benchmark_runtime_rga_insert_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_rga_insert_concurrent");
    group.sample_size(10);

    let context_id = default_context_id();
    let executor = default_executor();

    for thread_count in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let mut handles = Vec::new();

                    for t in 0..thread_count {
                        let context_id = context_id;
                        let executor = executor;

                        let handle = std::thread::spawn(move || {
                            // Each thread compiles its own module
                            let module = compile_benchmark_module();
                            let mut storage = InMemoryStorage::default();
                            init_app(&module, &mut storage);

                            // Each thread inserts 100 characters
                            for i in 0..100 {
                                let text = format!("t{}_i{}", t, i);
                                let input = json_input_multi(&[("index", "0"), ("text", &text)]);
                                black_box(
                                    module
                                        .run(
                                            context_id,
                                            executor,
                                            "rga_insert",
                                            &input,
                                            &mut storage,
                                            None,
                                            None,
                                        )
                                        .unwrap(),
                                );
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
        // Output directory for reports (relative to workspace root)
        .output_directory(Path::new("target/criterion/runtime-benchmarks"))
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
        benchmark_runtime_map_insert,
        benchmark_runtime_map_get,
        benchmark_runtime_map_remove,
        benchmark_runtime_map_contains,
        benchmark_runtime_nested_map_insert,
        benchmark_runtime_deep_nested_map_insert,
        benchmark_runtime_vector_push,
        benchmark_runtime_vector_get,
        benchmark_runtime_vector_pop,
        benchmark_runtime_set_insert,
        benchmark_runtime_set_contains,
        benchmark_runtime_counter_increment,
        benchmark_runtime_counter_get,
        benchmark_runtime_register_set,
        benchmark_runtime_register_get,
        benchmark_runtime_rga_insert,
        benchmark_runtime_rga_get_text
}

criterion_group! {
    name = multi_threaded;
    config = configure_criterion();
    targets =
        benchmark_runtime_map_insert_concurrent,
        benchmark_runtime_vector_push_concurrent,
        benchmark_runtime_set_insert_concurrent,
        benchmark_runtime_counter_increment_concurrent,
        benchmark_runtime_register_set_concurrent,
        benchmark_runtime_rga_insert_concurrent,
        benchmark_runtime_nested_map_insert_concurrent,
        benchmark_runtime_deep_nested_map_insert_concurrent
}

criterion_main!(single_threaded, multi_threaded);
