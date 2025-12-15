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

- `storage_map_insert` - UnorderedMap insert operations
- `storage_map_get` - UnorderedMap get operations
- `storage_map_remove` - UnorderedMap remove operations
- `storage_nested_map_insert` - Nested map operations (2 levels)
- `storage_deep_nested_map_insert` - Deep nested map operations (3 levels)
- `storage_vector_push` - Vector push operations
- `storage_vector_get` - Vector get by index
- `storage_vector_pop` - Vector pop operations
- `storage_set_insert` - UnorderedSet insert operations
- `storage_set_contains` - UnorderedSet contains check
- `storage_counter_increment` - Counter increment operations
- `storage_counter_get` - Counter get value
- `storage_register_set` - LwwRegister set operations
- `storage_register_get` - LwwRegister get operations
- `storage_rga_insert` - ReplicatedGrowableArray insert text
- `storage_rga_get_text` - ReplicatedGrowableArray get full text

### Multi-Threaded Benchmarks

- `storage_map_insert_concurrent` - Concurrent map inserts (2, 4, 8 threads)
- `storage_vector_push_concurrent` - Concurrent vector pushes (2, 4, 8 threads)
- `storage_set_insert_concurrent` - Concurrent set inserts (2, 4, 8 threads)

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

## Notes

- **Direct CRDT Operations**: Benchmarks call collection methods directly (no WASM/runtime layer)
- **Storage**: Uses default `MainStorage` (InMemoryStorage) for fast, deterministic benchmarks
- **Multi-threaded**: Each thread has its own collection instance (no sharing)
- **Reports**: HTML reports are generated automatically (no additional setup needed)
- **Baselines**: Save baselines to track performance changes over time