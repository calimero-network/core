# Node Performance

Performance characteristics and optimization guide for Calimero Node.

---

## Overview

The node layer adds ~1-5ms overhead on top of DAG operations for WASM execution, networking, and event handling.

---

## Latency Breakdown

### End-to-End Delta Propagation

```
Transaction → Broadcast → Reception → Application → Event Handlers → WebSocket
     5-50ms      ~100ms       <1ms        5-50ms          5-50ms        <1ms

Total: 15-150ms typical (100-200ms p99)
```

**Components**:

| Phase              | Time   | What Happens                     |
|--------------------|--------|----------------------------------|
| **Transaction**    | 5-50ms | WASM execution + CRDT operations |
| **Broadcast**      | ~100ms | Gossipsub propagation            |
| **Reception**      | <1ms   | Deserialize + DAG lookup         |
| **Application**    | 5-50ms | WASM `__calimero_sync_next`      |
| **Event Handlers** | 5-50ms | WASM event handler execution     |
| **WebSocket**      | <1ms   | Emit to connected clients        |

---

## Throughput

### Deltas per Second (per context)

| Network Size    | Throughput | Bottleneck          |
|-----------------|------------|---------------------|
| **Single node** | 1000/sec   | WASM execution      |
| **10 nodes**    | 100/sec    | Network + WASM      |
| **50 nodes**    | 50/sec     | Gossipsub fanout    |
| **100+ nodes**  | 20/sec     | Network saturation  |

### Contexts per Node

| Contexts  | Memory | CPU  | Notes              |
|-----------|--------|------|--------------------|
| **10**    | 100 MB | 5%   | Light load         |
| **100**   | 1 GB   | 20%  | Typical production |
| **500**   | 5 GB   | 50%  | High load          |
| **1000+** | 10GB+  | 80%+ | Requires tuning    |

---

## Memory Usage

### Per-Context Memory

```
DeltaStore: ~10 MB
  - DAG: 5-8 MB (1000 deltas)
  - Applied set: 32 KB
  - Pending: 0-500 KB

Total: ~10 MB per context
```

### Node-Wide Memory

```
100 contexts × 10 MB = 1 GB (DAGs)
Blob cache: 500 MB (configurable)
Arbiters: 50 MB (thread overhead)
Network: 100 MB (buffers)
Base: 50 MB (runtime)

Total: ~1.7 GB for 100 contexts
```

---

## CPU Usage

### Breakdown

| Component           | % of CPU | Notes                  |
|---------------------|----------|------------------------|
| **WASM Execution**  | 60%      | Dominant cost          |
| **Serialization**   | 15%      | Borsh encode/decode    |
| **DAG Operations**  | 10%      | Hash lookups, cascade  |
| **Network**         | 10%      | Gossipsub + P2P        |
| **Other**           | 5%       | Timers, logging        |

### Optimization

```rust
// Use release builds for WASM
cargo build --release --target wasm32-unknown-unknown

// Profile WASM execution
RUST_LOG=calimero_runtime=debug cargo run
```

---

## Network Bandwidth

### Gossipsub Overhead

```
Per delta broadcast:
  - Delta size: ~5 KB (typical)
  - Network overhead: ~1 KB (framing)
  - Total: ~6 KB

For 50 deltas/sec:
  - 50 × 6 KB = 300 KB/sec
  - ~2.4 Mbps
```

### P2P Sync Overhead

```
Per sync operation:
  - Heads exchange: ~1 KB
  - Delta transfer: 5 KB × delta_count
  - Snapshot: up to 10 MB

Frequency: Every 10-30s per context
```

---

## Blob Cache Performance

### Hit Rate

| Scenario             | Hit Rate | Notes               |
|----------------------|----------|---------------------|
| **Static content**   | 90%+     | Images, assets      |
| **Dynamic content**  | 50-70%   | Frequently changing |
| **One-time access**  | 0%       | Downloads           |

### Eviction Strategy

```
Phase 1: Age-based (5 min TTL)
  → Removes 70% of stale blobs

Phase 2: Count-based (100 max)
  → Removes 20% if over limit

Phase 3: Size-based (500 MB max)
  → Removes 10% if over budget

Total eviction time: ~2 ms for 100 blobs
```

---

## Periodic Timer Overhead

| Timer              | Frequency | CPU | Memory           |
|--------------------|-----------|-----|------------------|
| **Blob eviction**  | 5 min     | <1% | Frees 100-500 MB |
| **Delta cleanup**  | 60 sec    | <1% | Frees 0-5 MB     |
| **Hash heartbeat** | 30 sec    | <1% | ~1 KB/broadcast  |

**Total overhead**: <3% CPU, minimal impact

---

## Benchmarks

### Single-Node Performance

**Environment**: M1 MacBook Pro, 16GB RAM

```
Transaction throughput: 1000/sec
Delta application: ~5 ms/delta
Event handler: ~10 ms/handler
Memory per context: ~10 MB
```

### Multi-Node Performance

**Environment**: 20 nodes, AWS t3.medium

```
Gossipsub latency: 100-150 ms (p50)
Gossipsub latency: 200-300 ms (p99)
Sync latency: 500-1000 ms
Convergence time: 2-5 sec (after partition)
```

---

## Optimization Guide

### 1. Reduce WASM Execution Time

```rust
// Minimize allocations in hot paths
pub fn process_batch(items: &[Item]) {
    // ✅ Good: Pre-allocate
    let mut results = Vec::with_capacity(items.len());
    
    // ❌ Bad: Reallocate on every push
    let mut results = Vec::new();
}
```

### 2. Batch Operations

```rust
// ✅ Good: Batch deltas
let deltas = vec![delta1, delta2, delta3];
for delta in deltas {
    dag.add_delta(delta, &applier).await?;
}

// ❌ Bad: Individual adds with delays
dag.add_delta(delta1, &applier).await?;
tokio::time::sleep(Duration::from_millis(10)).await;
dag.add_delta(delta2, &applier).await?;
```

### 3. Tune Sync Configuration

```rust
// For high-throughput contexts
let config = SyncConfig {
    interval: Duration::from_secs(2),  // Fast recovery
    max_concurrent: 50,                 // High parallelism
    ..Default::default()
};
```

### 4. Optimize Blob Cache

```rust
// Increase cache for static content
const MAX_CACHE_COUNT: usize = 200;
const MAX_CACHE_BYTES: usize = 1024 * 1024 * 1024;  // 1 GB
```

### 5. Profile and Monitor

```rust
// Add metrics
metrics::histogram!("node.delta_apply_duration", duration.as_secs_f64());
metrics::counter!("node.events_executed", 1);
metrics::gauge!("node.pending_deltas", stats.count as f64);
```

---

## Scaling Recommendations

### Small Deployment (< 10 nodes)

**Resources**:
- 2 GB RAM
- 2 CPU cores
- 100 Mbps network

**Configuration**:
```rust
let config = SyncConfig {
    interval: Duration::from_secs(2),
    max_concurrent: 10,
    ..Default::default()
};
```

### Medium Deployment (10-50 nodes)

**Resources**:
- 8 GB RAM
- 4 CPU cores
- 1 Gbps network

**Configuration**:
```rust
SyncConfig::default()  // Use defaults
```

### Large Deployment (50-200 nodes)

**Resources**:
- 16 GB RAM
- 8 CPU cores
- 10 Gbps network

**Configuration**:
```rust
let config = SyncConfig {
    interval: Duration::from_secs(15),
    frequency: Duration::from_secs(30),
    max_concurrent: 20,
    ..Default::default()
};
```

---

## See Also

- [Sync Configuration](sync-configuration.md) - Tuning sync parameters
- [Architecture](architecture.md) - Component design
- [DAG Performance](../../dag/readme/performance.md) - DAG-specific performance

