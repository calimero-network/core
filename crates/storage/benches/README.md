# Storage Benchmarks

This directory contains Criterion.rs benchmarks for measuring pure CRDT collection performance in the `calimero-storage` crate.

## Overview

These benchmarks measure the storage layer directly, without WASM or runtime overhead:

- **CRDT Operations**: Direct collection method calls (insert, get, remove, etc.)
- **Serialization/Deserialization**: Borsh encoding/decoding overhead
- **Storage I/O**: Read/write operations through storage interface
- **No WASM Overhead**: Pure Rust performance

## Prerequisites

No special setup needed. The benchmarks use `Root::new()` to create collections directly.

## Running Benchmarks

### Run All Benchmarks

```bash
cd crates/storage
cargo bench
```

This will:

- Run all benchmarks
- Generate HTML reports with interactive plots
- Save results to `target/criterion/storage-benchmarks/`

### Run Specific Benchmark Group

```bash
# Single-threaded benchmarks only
cargo bench --bench collections -- single_threaded

# Multi-threaded benchmarks only
cargo bench --bench collections -- multi_threaded

# Value size variation benchmarks
cargo bench --bench collections -- value_sizes

# Edge case benchmarks
cargo bench --bench collections -- edge_cases

# Memory/space benchmarks
cargo bench --bench collections -- memory
```

### Run Specific Benchmark

```bash
# Run only map insert benchmarks
cargo bench --bench collections -- map_insert

# Run only concurrent benchmarks
cargo bench --bench collections -- concurrent
```

## Benchmark Categories

### Single-Threaded Benchmarks

**Basic Operations:**

- `storage_map_insert` - UnorderedMap insert operations
- `storage_map_get` - UnorderedMap get operations
- `storage_map_remove` - UnorderedMap remove operations
- `storage_map_contains` - UnorderedMap contains check
- `storage_map_iter` - UnorderedMap iteration
- `storage_nested_map_insert` - Nested map operations (2 levels)
- `storage_deep_nested_map_insert` - Deep nested map operations (3 levels)
- `storage_vector_push` - Vector push operations
- `storage_vector_get` - Vector get by index
- `storage_vector_pop` - Vector pop operations
- `storage_vector_iter` - Vector iteration
- `storage_set_insert` - UnorderedSet insert operations
- `storage_set_contains` - UnorderedSet contains check
- `storage_set_iter` - UnorderedSet iteration
- `storage_counter_increment` - Counter increment operations
- `storage_counter_get` - Counter get value
- `storage_register_set` - LwwRegister set operations
- `storage_register_get` - LwwRegister get operations
- `storage_rga_insert` - ReplicatedGrowableArray insert text
- `storage_rga_get_text` - ReplicatedGrowableArray get full text
- `storage_rga_delete` - ReplicatedGrowableArray delete at position
- `storage_rga_delete_range` - ReplicatedGrowableArray delete range

**CRDT Operations:**

- `storage_map_merge` - UnorderedMap merge operations
- `storage_vector_merge` - Vector merge operations
- `storage_set_merge` - UnorderedSet merge operations
- `storage_counter_merge` - Counter merge operations
- `storage_register_merge` - LwwRegister merge operations
- `storage_rga_merge` - ReplicatedGrowableArray merge operations

**Serialization:**

- `storage_map_serialize` / `deserialize` - Map serialization/deserialization
- `storage_vector_serialize` / `deserialize` - Vector serialization/deserialization
- `storage_set_serialize` / `deserialize` - Set serialization/deserialization
- `storage_counter_serialize` / `deserialize` - Counter serialization/deserialization
- `storage_register_serialize` / `deserialize` - Register serialization/deserialization
- `storage_rga_serialize` / `deserialize` - RGA serialization/deserialization

### Multi-Threaded Benchmarks

- `storage_map_insert_concurrent` - Concurrent map inserts (2, 4, 8 threads)
- `storage_vector_push_concurrent` - Concurrent vector pushes (2, 4, 8 threads)
- `storage_set_insert_concurrent` - Concurrent set inserts (2, 4, 8 threads)
- `storage_counter_increment_concurrent` - Concurrent counter increments (2, 4, 8 threads)
- `storage_register_set_concurrent` - Concurrent register sets (2, 4, 8 threads)
- `storage_rga_insert_concurrent` - Concurrent RGA inserts (2, 4, 8 threads)

### Value Size Variation Benchmarks

Tests performance with different value sizes:

- `storage_map_insert_value_sizes` - Map insert with small/medium/large/very_large values
- `storage_vector_push_value_sizes` - Vector push with different value sizes
- `storage_set_insert_value_sizes` - Set insert with different value sizes

**Value Sizes:**

- Small: 10 bytes
- Medium: 100 bytes
- Large: 1KB
- Very Large: 10KB

### Edge Case Benchmarks

Tests edge cases and boundary conditions:

- `storage_map_empty_operations` - Operations on empty map (first insert, get, contains)
- `storage_vector_empty_operations` - Operations on empty vector (first push, get, pop)
- `storage_map_single_element` - Operations with single element in map
- `storage_vector_single_element` - Operations with single element in vector

### Memory/Space Benchmarks

Tracks memory usage and space efficiency:

- `storage_map_memory_per_element` - Memory usage per map entry (bytes/element)
- `storage_vector_memory_per_element` - Memory usage per vector element (bytes/element)

**Note:** Memory benchmarks report bytes per element in stderr for analysis.

## Understanding Results

Criterion.rs provides:

- **Time per operation**: ns/µs/ms
- **Throughput**: ops/sec
- **Statistical analysis**: mean, median, p90, p99
- **Outlier detection**: Automatic detection and reporting

## Comparing Results

### Save Baseline

```bash
cargo bench -- --save-baseline baseline
```

### Compare Against Baseline

```bash
cargo bench -- --baseline baseline
```

This will show performance changes compared to the saved baseline.

## Size Ranges

All benchmarks test multiple size ranges:

- **Storage Benchmarks**: `[10, 100, 1_000, 10_000, 100_000]` elements
- Defined as `STORAGE_BENCHMARK_SIZES` constant for easy modification

## Notes

- **Direct CRDT Operations**: Benchmarks call collection methods directly (no WASM/runtime layer)
- **Storage**: Uses default `MainStorage` (InMemoryStorage) for fast, deterministic benchmarks
- **Multi-threaded**: Each thread has its own collection instance (no sharing)
- **Reports**: HTML reports are generated automatically (no additional setup needed)
- **Baselines**: Save baselines to track performance changes over time
- **Merge Operations**: Use `LwwRegister<String>` as values since `String` doesn't implement `Mergeable`
