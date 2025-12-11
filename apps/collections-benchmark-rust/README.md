# Collections Benchmark (Rust SDK)

Performance benchmarking application for Calimero CRDT collections using the Rust SDK.

## Overview

This app measures the performance of CRDT collection operations to:

- Compare Rust SDK vs JavaScript SDK performance
- Analyze performance across different collection sizes
- Measure impact of nesting levels on performance
- Track performance regressions over time

## Collection Types Benchmarked

| Collection                | Operations                                         | Nesting Levels |
| ------------------------- | -------------------------------------------------- | -------------- |
| `Counter`                 | increment                                          | 1              |
| `PNCounter`               | increment, decrement, mixed                        | 1              |
| `UnorderedMap<K, V>`      | insert, get                                        | 1, 2, 3        |
| `Vector<T>`               | push, get                                          | 1              |
| `UnorderedSet<T>`         | insert, contains                                   | 1              |
| `LwwRegister<T>`          | set, get                                           | 1              |
| `ReplicatedGrowableArray` | insert, insert_str, get_text, delete, delete_range | 1              |

## Size Categories

- **Small**: 10-100 elements
- **Medium**: 1,000-10,000 elements
- **Large**: 100,000+ elements

## Nesting Levels

- **Level 1**: Simple collections (`UnorderedMap<String, Counter>`)
- **Level 2**: Nested maps (`UnorderedMap<String, UnorderedMap<String, Counter>>`)
- **Level 3**: Deep nesting (`UnorderedMap<String, UnorderedMap<String, UnorderedMap<String, Counter>>>`)

## Benchmark Methods

### Individual Benchmarks

```rust
// Counter
benchmark_counter_increment(size: u32) -> String

// PNCounter
benchmark_pncounter_increment(size: u32) -> String
benchmark_pncounter_decrement(size: u32) -> String
benchmark_pncounter_mixed(size: u32) -> String

// UnorderedMap (Level 1)
benchmark_map_insert(size: u32) -> String
benchmark_map_get(size: u32) -> String

// UnorderedMap (Level 2 - Nested)
benchmark_nested_map_insert(size: u32) -> String
benchmark_nested_map_get(size: u32) -> String

// UnorderedMap (Level 3 - Deep Nested)
benchmark_deep_nested_map_insert(size: u32) -> String

// Vector
benchmark_vector_push(size: u32) -> String
benchmark_vector_get(size: u32) -> String

// UnorderedSet
benchmark_set_insert(size: u32) -> String
benchmark_set_contains(size: u32) -> String

// LwwRegister
benchmark_register_set(size: u32) -> String
benchmark_register_get(size: u32) -> String

// ReplicatedGrowableArray (RGA)
benchmark_rga_insert(size: u32) -> String
benchmark_rga_insert_str(size: u32) -> String
benchmark_rga_get_text(size: u32) -> String
benchmark_rga_delete(size: u32) -> String
benchmark_rga_delete_range(size: u32) -> String
```

### Comprehensive Suite

```rust
// Run all benchmarks for a given size
run_benchmark_suite(size: u32) -> String
```

### Utility Methods

```rust
// Get the number of benchmark runs
get_run_count() -> Result<u64, String>

// Store a benchmark result for later retrieval
store_result(key: String, result: String) -> Result<(), String>

// Retrieve a stored benchmark result
get_stored_result(key: String) -> Result<Option<String>, String>
```

