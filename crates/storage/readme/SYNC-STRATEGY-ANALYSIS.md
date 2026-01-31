# Calimero Sync Strategy Performance Analysis

**Date**: January 31, 2026  
**Version**: 1.0  
**Authors**: Calimero Core Team

---

## Executive Summary

This document presents a comparative analysis of four Merkle tree state synchronization strategies implemented in the Calimero distributed state system. Using controlled benchmarks with the `--force-state-sync` flag, we measured actual strategy performance in isolation from the DAG-based delta propagation layer.

### Key Findings

| Strategy | Avg Latency | Round Trips | Best Use Case |
|----------|-------------|-------------|---------------|
| **Bloom Filter** | **1.38ms** | **1** | Small divergence (<10%) |
| **Level-Wise** | **2.70ms** | **2** | Wide, shallow trees |
| Subtree Prefetch | 13.13ms | 26.4 | Deep trees, localized changes |
| Hash Comparison | 13.94ms | 27.0 | General purpose, always correct |

**Bloom Filter achieves 10x lower latency than Hash Comparison** with 27x fewer network round trips for the tested workload.

---

## 1. Introduction

### 1.1 Background

Calimero uses a DAG-based CRDT system with Merkle tree state verification. When nodes diverge, they must synchronize state. The system supports multiple strategies:

1. **DAG Catchup** (default): Fetch missing deltas from DAG history
2. **State Sync Strategies**: Direct Merkle tree comparison when DAG is unavailable or insufficient

### 1.2 Problem Statement

Prior to this analysis, state sync strategies (Bloom, Hash, Subtree, Level-Wise) were implemented but never benchmarked in isolation because DAG catchup always took precedence when DAG history was available.

### 1.3 Solution

We added a `--force-state-sync` flag that bypasses DAG catchup, allowing direct benchmarking of state sync strategies in divergence scenarios.

---

## 2. Methodology

### 2.1 Test Environment

| Parameter | Value |
|-----------|-------|
| Nodes | 2 |
| Network | Local (loopback) |
| State Size | 10 keys |
| Key Size | ~50 bytes |
| Value Size | ~100 bytes |
| Tree Depth | ~2 levels |
| merod Version | 0.1.0 (release build) |

### 2.2 Test Scenario

1. Node 1 creates context with application
2. Node 2 joins context
3. **Node 2 stops** (simulating partition)
4. **Node 1 writes 10 keys** while Node 2 is down
5. **Node 2 restarts** with `--force-state-sync` flag
6. Node 2 syncs using configured strategy
7. Verify all keys present on Node 2

### 2.3 Metrics Collected

From `STRATEGY_SYNC_METRICS` log markers:

- `duration_ms`: Total sync operation time
- `round_trips`: Network round trips required
- `entities_synced`: Number of entities transferred
- `entities_skipped`: Entities already present locally
- `bytes_received`: Network bytes received
- `bytes_sent`: Network bytes sent
- Strategy-specific metrics (bloom_filter_size, nodes_checked, etc.)

### 2.4 Statistical Method

- Each strategy ran for ~65 seconds
- 30-36 sync operations per strategy
- Metrics: Average (mean) reported

---

## 3. Results

### 3.1 Performance Comparison

| Strategy | Syncs (n) | Avg Duration (ms) | Avg Round Trips | Speedup vs Hash |
|----------|-----------|-------------------|-----------------|-----------------|
| Bloom Filter | 30 | **1.38** | **1.0** | **10.1x** |
| Level-Wise | 34 | 2.70 | 2.0 | 5.2x |
| Subtree Prefetch | 36 | 13.13 | 26.4 | 1.1x |
| Hash Comparison | 34 | 13.94 | 27.0 | 1.0x (baseline) |

### 3.2 Round Trip Analysis

```
Round Trips per Strategy (10-key workload):

Bloom Filter   │█ 1
Level-Wise     │██ 2
Subtree        │██████████████████████████ 26
Hash Compare   │███████████████████████████ 27
               └─────────────────────────────
                0        10        20       30
```

### 3.3 Strategy-Specific Metrics

#### Bloom Filter
```
bloom_filter_size: 25 bytes
false_positive_rate: 0.01 (1%)
local_entity_count: 16
matched_count: 16
```

#### Hash Comparison
```
nodes_checked: ~27
max_depth_reached: 2
hash_comparisons: ~27
```

#### Subtree Prefetch
```
subtrees_fetched: ~26
divergent_children: varies
prefetch_depth: 255 (max)
```

#### Level-Wise
```
levels_synced: 2
max_nodes_per_level: 1
total_nodes_checked: ~2
```

---

## 4. Analysis

### 4.1 Why Bloom Filter Wins

Bloom Filter achieves O(1) round trips regardless of tree size:

1. **Send Phase**: Build Bloom filter of local entity IDs, send to peer (25 bytes)
2. **Receive Phase**: Peer checks their entities against filter, returns missing ones
3. **Apply Phase**: Apply received entities locally

**Trade-off**: False positives (1% at default rate) may cause unnecessary entity transfers.

### 4.2 Why Level-Wise is Second Best

Level-Wise batches requests by tree depth:

1. Request root level → get all children
2. Request next level → get divergent children's children
3. Repeat until leaves

For shallow trees (depth=2), this is O(2) round trips.

**Trade-off**: Performance degrades with tree depth.

### 4.3 Why Hash/Subtree are Similar

Both traverse the tree recursively, checking each node:

- **Hash Comparison**: Request node, compare hash, recurse if different
- **Subtree Prefetch**: Same traversal, but fetches entire subtrees when divergent

With 10 keys and shallow depth, both make ~27 round trips (one per tree node).

**Trade-off**: Subtree Prefetch would win with deeper trees and localized changes.

---

## 5. Complexity Analysis

| Strategy | Time Complexity | Round Trips | Best Case | Worst Case |
|----------|-----------------|-------------|-----------|------------|
| Bloom Filter | O(n) | O(1) | Small divergence | High false positives |
| Level-Wise | O(n) | O(depth) | Shallow wide trees | Deep narrow trees |
| Subtree Prefetch | O(n) | O(divergent_subtrees) | Localized changes | Random scattered changes |
| Hash Comparison | O(n) | O(tree_nodes) | Perfect baseline | Always correct |

Where:
- n = number of entities
- depth = tree depth
- tree_nodes = total nodes in Merkle tree

---

## 6. Recommendations

### 6.1 Strategy Selection Guidelines

| Scenario | Recommended Strategy | Reason |
|----------|---------------------|--------|
| Unknown divergence | **Adaptive** | Auto-selects based on tree characteristics |
| Small divergence (<10%) | **Bloom Filter** | O(1) round trips |
| Large state, small diff | **Bloom Filter** | Efficient diff detection |
| Wide shallow tree | **Level-Wise** | Batches by level |
| Deep tree, local changes | **Subtree Prefetch** | Fetches whole subtrees |
| Safety-critical | **Hash Comparison** | No false positives |

### 6.2 Production Defaults

```rust
// Recommended production configuration
StateSyncStrategy::Adaptive {
    bloom_filter_threshold: 50,      // Use bloom if > 50 entities
    subtree_prefetch_depth: 3,       // Use subtree if depth > 3
    snapshot_divergence_threshold: 0.5, // Use snapshot if > 50% divergence
}
```

### 6.3 Benchmarking Flag

For testing state sync strategies in isolation:

```bash
merod run --state-sync-strategy bloom --force-state-sync
```

**⚠️ Warning**: `--force-state-sync` bypasses DAG catchup and should only be used for benchmarking.

---

## 7. Limitations

### 7.1 Current Study Limitations

1. **Small workload**: 10 keys is insufficient to stress strategies
2. **Shallow tree**: Depth of 2 favors Level-Wise
3. **Local network**: No real network latency
4. **No concurrent writes**: Single-writer scenario

### 7.2 Future Work

1. **Scale testing**: 1K, 10K, 100K keys
2. **Tree shape variation**: Deep narrow, wide shallow, balanced
3. **Network conditions**: Add latency, packet loss
4. **Concurrent writers**: Multi-node conflict scenarios
5. **False positive analysis**: Measure Bloom filter accuracy at scale

---

## 8. Reproducing Results

### 8.1 Prerequisites

```bash
# Build release binary
cargo build --release -p merod

# Ensure merobox is installed
pip install -e ./merobox
```

### 8.2 Run Benchmark

```bash
# Full strategy comparison
./scripts/benchmark-sync-strategies.sh ./target/release/merod

# Single strategy test
python -m merobox.cli bootstrap run --no-docker \
  --binary-path ./target/release/merod \
  --merod-args="--state-sync-strategy bloom --force-state-sync" \
  workflows/sync/test-bloom-filter.yml
```

### 8.3 Extract Metrics

```bash
# Extract from logs
./scripts/extract-sync-metrics.sh <prefix>

# View specific strategy logs
grep "STRATEGY_SYNC_METRICS" data/<prefix>-2/logs/*.log
```

---

## 9. Raw Data

### 9.1 Bloom Filter

| Metric | Value |
|--------|-------|
| Syncs | 30 |
| Avg Duration | 1.38ms |
| Avg Round Trips | 1.0 |
| Filter Size | 25 bytes |
| False Positive Rate | 1% |

### 9.2 Hash Comparison

| Metric | Value |
|--------|-------|
| Syncs | 34 |
| Avg Duration | 13.94ms |
| Avg Round Trips | 27.0 |
| Nodes Checked | ~27 |
| Max Depth | 2 |

### 9.3 Subtree Prefetch

| Metric | Value |
|--------|-------|
| Syncs | 36 |
| Avg Duration | 13.13ms |
| Avg Round Trips | 26.4 |
| Subtrees Fetched | ~26 |
| Prefetch Depth | 255 |

### 9.4 Level-Wise

| Metric | Value |
|--------|-------|
| Syncs | 34 |
| Avg Duration | 2.70ms |
| Avg Round Trips | 2.0 |
| Levels Synced | 2 |
| Max Nodes/Level | 1 |

---

## 10. Conclusion

This analysis demonstrates that **Bloom Filter sync achieves 10x better latency** than recursive hash comparison for small divergence scenarios, at the cost of potential false positives. **Level-Wise sync provides a 5x improvement** for shallow trees.

The `--force-state-sync` flag enables proper benchmarking of these strategies, which was previously impossible due to DAG catchup always taking precedence.

For production, the **Adaptive strategy** is recommended, which automatically selects the optimal protocol based on tree characteristics.

---

## Appendix A: Log Markers

### STRATEGY_SYNC_METRICS

```
STRATEGY_SYNC_METRICS 
  context_id=<id>
  peer_id=<peer>
  strategy="bloom_filter|hash_comparison|subtree_prefetch|level_wise"
  round_trips=<n>
  entities_synced=<n>
  entities_skipped=<n>
  bytes_received=<n>
  bytes_sent=<n>
  duration_ms="<ms>"
  # Strategy-specific fields...
```

### Bloom Filter Specific

```
bloom_filter_size=<bytes>
false_positive_rate=<rate>
local_entity_count=<n>
matched_count=<n>
```

### Hash Comparison Specific

```
nodes_checked=<n>
max_depth_reached=<n>
hash_comparisons=<n>
```

### Subtree Prefetch Specific

```
subtrees_fetched=<n>
divergent_children=<n>
total_children=<n>
prefetch_depth=<n>
```

### Level-Wise Specific

```
levels_synced=<n>
max_nodes_per_level=<n>
total_nodes_checked=<n>
configured_max_depth=<n>
```

---

## Appendix B: CLI Reference

```bash
# Fresh node strategy (for uninitialized nodes)
--sync-strategy snapshot|delta|adaptive

# State sync strategy (for initialized nodes with divergence)
--state-sync-strategy adaptive|hash|snapshot|compressed|bloom|subtree|level

# Force state sync (bypass DAG catchup for benchmarking)
--force-state-sync
```

---

*Document generated from benchmark run on 2026-01-31*
