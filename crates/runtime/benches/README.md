# Runtime Benchmarks

This directory contains Criterion.rs benchmarks for measuring WASM execution performance through the Calimero runtime.

## Overview

These benchmarks measure the full stack:

- **WASM Execution**: Compilation and execution of WASM modules
- **Host Functions**: Overhead of WASM → Rust bridge
- **CRDT Operations**: Through runtime interface
- **Storage I/O**: Through runtime storage interface

## Prerequisites

Before running benchmarks, you need to build the benchmark WASM module:

```bash
# From workspace root
cd apps/collections-benchmark-rust
cargo build --release --target wasm32-unknown-unknown

# The WASM file should be at:
# apps/collections-benchmark-rust/res/collections_benchmark_rust.wasm
```

## Running Benchmarks

### Run All Benchmarks

```bash
cd crates/runtime
cargo bench
```

This will:

- Run all benchmarks
- Generate HTML reports with interactive plots
- Save results to `target/criterion/runtime-benchmarks/`

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

### CSV Export

Criterion automatically generates CSV files for data export:

```bash
# CSV files are located at:
target/criterion/runtime-benchmarks/*/new/raw.csv
target/criterion/runtime-benchmarks/*/baseline/raw.csv  # If baseline exists
```

**CSV Format:**

- One row per sample
- Columns: iteration, measured_time_ns, etc.
- Can be imported into Excel, Python pandas, R, etc.

### JSON Export (for CI/CD)

To export results as JSON for CI/CD pipelines:

```bash
# Run with JSON output
cargo bench --bench collections -- --output-format json > results.json
```

Or use Criterion's built-in export:

```bash
# Export specific benchmark
cargo bench --bench collections -- map_insert -- --export-format json
```

### Comparing Baselines

Save a baseline for comparison:

```bash
# Save current run as baseline
cargo bench --bench collections -- --save-baseline my_baseline

# Compare against baseline
cargo bench --bench collections -- --baseline my_baseline
```

The HTML reports will show:

- **Performance change**: Percentage improvement/regression
- **Statistical significance**: Whether the change is meaningful
- **Visual comparison**: Side-by-side plots

## Benchmark Categories

### Single-Threaded Benchmarks

**Basic Operations:**

- `runtime_map_insert` - UnorderedMap insert operations
- `runtime_map_get` - UnorderedMap get operations
- `runtime_map_remove` - UnorderedMap remove operations
- `runtime_map_contains` - UnorderedMap contains check
- `runtime_nested_map_insert` - Nested map operations (2 levels)
- `runtime_deep_nested_map_insert` - Deep nested map operations (3 levels)
- `runtime_vector_push` - Vector push operations
- `runtime_vector_get` - Vector get operations
- `runtime_vector_pop` - Vector pop operations
- `runtime_set_insert` - UnorderedSet insert operations
- `runtime_set_contains` - UnorderedSet contains check
- `runtime_counter_increment` - Counter increment operations
- `runtime_counter_get` - Counter get value
- `runtime_register_set` - LwwRegister set operations
- `runtime_register_get` - LwwRegister get operations
- `runtime_rga_insert` - ReplicatedGrowableArray insert operations
- `runtime_rga_get_text` - ReplicatedGrowableArray get full text

### Multi-Threaded Benchmarks

- `runtime_map_insert_concurrent` - Concurrent map inserts (2, 4, 8 threads)
- `runtime_vector_push_concurrent` - Concurrent vector pushes (2, 4, 8 threads)
- `runtime_set_insert_concurrent` - Concurrent set inserts (2, 4, 8 threads)
- `runtime_counter_increment_concurrent` - Concurrent counter increments (2, 4, 8 threads)
- `runtime_register_set_concurrent` - Concurrent register sets (2, 4, 8 threads)
- `runtime_rga_insert_concurrent` - Concurrent RGA inserts (2, 4, 8 threads)
- `runtime_nested_map_insert_concurrent` - Concurrent nested map inserts (2, 4, 8 threads)
- `runtime_deep_nested_map_insert_concurrent` - Concurrent deep nested map inserts (2, 4, 8 threads)

## Understanding Results

Criterion.rs provides:

- **Time per operation**: ns/µs/ms
- **Throughput**: ops/sec
- **Statistical analysis**: mean, median, p90, p99
- **Outlier detection**: Automatic detection and reporting

### Example Output

```
runtime_map_insert/10     time:   [1.234 ms 1.256 ms 1.280 ms]
                        thrpt:  [7.8125 Kelem/s 7.9618 Kelem/s 8.1030 Kelem/s]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe
```

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

- **Runtime Benchmarks**: `[10, 100, 1_000]` elements
- Defined as `RUNTIME_BENCHMARK_SIZES` constant for easy modification

## Notes

- **WASM Compilation**: Each benchmark group compiles the WASM module once (expensive but necessary)
- **Storage**: Uses `InMemoryStorage` for fast, deterministic benchmarks
- **Multi-threaded**: Each thread has its own storage instance (no sharing)
- **Context**: Uses default context ID and executor for all benchmarks
- **Reports**: HTML reports are generated automatically (no additional setup needed)
- **Baselines**: Save baselines to track performance changes over time
- **Sample Size**: Increased to 20 samples for better statistical confidence
- **Missing Operations**: Some operations (e.g., `map_remove`, `vector_pop`) may not exist in the WASM module and will fail if not implemented

## Troubleshooting

### WASM File Not Found

If you see:

```
WASM file not found at ../../apps/collections-benchmark-rust/res/collections_benchmark_rust.wasm
```

Build the WASM module first (see Prerequisites).

### Compilation Errors

If benchmarks fail to compile:

1. Ensure Criterion.rs is in `dev-dependencies`
2. Check that all required crates are available
3. Verify the WASM file path is correct

