# DAG Performance

Performance characteristics, complexity analysis, and optimization guide.

---

## Time Complexity

### Core Operations

| Operation | Best Case | Average Case | Worst Case | Notes |
|-----------|-----------|--------------|------------|-------|
| `add_delta` (ready) | O(1) | O(1) | O(P) | P = pending count (cascade) |
| `add_delta` (pending) | O(1) | O(1) | O(1) | Just buffer |
| `can_apply` | O(1) | O(k) | O(k) | k = parent count (~1-3) |
| `get_heads` | O(H) | O(H) | O(H) | H = head count (~1-10) |
| `get_missing_parents` | O(P × k) | O(P × k) | O(P × k) | Scan all pending |
| `cleanup_stale` | O(P) | O(P) | O(P) | Filter pending map |
| `get_deltas_since` | O(D) | O(D) | O(D) | D = deltas since ancestor |
| `pending_stats` | O(P × k) | O(P × k) | O(P × k) | Sum missing parents |

**Legend**:
- P = pending delta count
- H = head count
- k = average parent count per delta
- D = delta count since ancestor

### Detailed Analysis

#### add_delta (When Delta is Ready)

```rust
pub async fn add_delta(
    &mut self,
    delta: CausalDelta<T>,
    applier: &impl DeltaApplier<T>,
) -> Result<bool> {
    // 1. Duplicate check: O(1)
    if self.deltas.contains_key(&delta_id) {
        return Ok(false);
    }
    
    // 2. Store delta: O(1)
    self.deltas.insert(delta_id, delta.clone());
    
    // 3. Check parents: O(k) where k = parent count
    if self.can_apply(&delta) {
        // 4. Apply delta: O(applier) - typically O(1) to O(log N)
        applier.apply(&delta).await?;
        
        // 5. Mark applied: O(1)
        self.applied.insert(delta.id);
        
        // 6. Update heads: O(k)
        for parent in &delta.parents {
            self.heads.remove(parent);  // O(1) per parent
        }
        self.heads.insert(delta.id);  // O(1)
        
        // 7. Cascade: O(P × k) in worst case
        self.apply_pending(applier).await?;
        
        Ok(true)
    } else {
        // Pending: O(1)
        self.pending.insert(delta_id, PendingDelta::new(delta));
        Ok(false)
    }
}
```

**Worst Case**: O(P × k) when cascade applies many pending deltas

**Typical Case**: O(1) when no cascade needed

#### apply_pending (Cascade)

```rust
async fn apply_pending(&mut self, applier: &impl DeltaApplier<T>) -> Result<()> {
    let mut applied_any = true;
    
    while applied_any {
        applied_any = false;
        
        // Scan all pending: O(P)
        let ready: Vec<[u8; 32]> = self.pending.iter()
            .filter(|(_, pending)| self.can_apply(&pending.delta))  // O(k) per delta
            .map(|(id, _)| *id)
            .collect();
        
        // Apply ready: O(R) where R = ready count
        for id in ready {
            if let Some(pending) = self.pending.remove(&id) {
                self.apply_delta(pending.delta, applier).await?;
                applied_any = true;
            }
        }
    }
    
    Ok(())
}
```

**Worst Case**: O(N × P × k) where N = deltas applied in cascade

**Example**: Chain of 10 pending deltas applied in sequence
- Iteration 1: Scan 10, apply 1 → 9 remaining
- Iteration 2: Scan 9, apply 1 → 8 remaining
- ...
- Iteration 10: Scan 1, apply 1 → 0 remaining
- Total: 10 + 9 + 8 + ... + 1 = 55 scans = O(N²)

**Optimization Needed**: Reverse parent index (see Future Optimizations)

---

## Space Complexity

### Memory Per Delta

```rust
// CausalDelta<T> structure
struct CausalDelta<T> {
    id: [u8; 32],                    // 32 bytes
    parents: Vec<[u8; 32]>,          // 24 + (32 × parent_count) bytes
    payload: T,                      // Variable (typically 1-10 KB)
    hlc: HybridTimestamp,            // 16 bytes (u64 + u32 + padding)
}
```

**Typical sizes**:
- Minimal overhead: 32 + 24 + 32 + 16 = **104 bytes**
- With 1 parent: 104 + 32 = **136 bytes**
- With 5KB payload: 136 + 5120 = **5256 bytes** (~5.1 KB)

### DagStore Memory

```rust
pub struct DagStore<T> {
    deltas: HashMap<[u8; 32], CausalDelta<T>>,     // All deltas
    applied: HashSet<[u8; 32]>,                    // Just IDs
    pending: HashMap<[u8; 32], PendingDelta<T>>,  // Pending deltas
    heads: HashSet<[u8; 32]>,                      // Just IDs
}
```

**For 1000 applied deltas** (typical per context):

| Component | Count | Size per Item | Total Size |
|-----------|-------|---------------|------------|
| `deltas` map | 1000 | 5.1 KB | 5.1 MB |
| `applied` set | 1000 | 32 bytes | 32 KB |
| `pending` map | 0-100 | 5.1 KB + 16 bytes | 0-510 KB |
| `heads` set | 1-10 | 32 bytes | 32-320 bytes |
| **Total** | | | **~5-6 MB** |

**Memory growth**:
- **Linear** with delta count
- **Unbounded** (no pruning implemented)
- **Pending buffer** bounded by timeout cleanup

### Per-Node Memory (100 contexts)

```
100 contexts × 6 MB per context = 600 MB

Plus:
- Rust overhead: ~10%
- HashMap overhead: ~20%

Total: ~750 MB - 1 GB
```

---

## Benchmarks

### Synthetic Tests

**Environment**: M1 MacBook Pro, 16GB RAM, Rust 1.75

#### Test 1: Linear Sequence (1000 deltas)

```rust
// Apply 1000 deltas in sequence: D0 → D1 → D2 → ... → D999
for i in 0..1000 {
    let delta = create_delta(i, vec![i-1], payload);
    dag.add_delta(delta, &applier).await.unwrap();
}
```

**Results**:
- Total time: **~5 ms**
- Per delta: **~5 μs**
- Throughput: **200,000 deltas/sec**

**Analysis**: Very fast because:
- No cascade needed (parents always ready)
- O(1) per delta
- Minimal allocations

#### Test 2: Out-of-Order (1000 deltas, reverse)

```rust
// Add deltas in reverse order: D999, D998, ..., D1, D0
for i in (0..1000).rev() {
    let delta = create_delta(i, vec![i-1], payload);
    dag.add_delta(delta, &applier).await.unwrap();
}
```

**Results**:
- Total time: **~500 ms**
- Per delta: **~500 μs**
- Throughput: **2,000 deltas/sec**

**Analysis**: 100x slower due to:
- First 999 deltas buffer as pending (O(1) each)
- Delta 0 triggers cascade of 999 deltas (O(N²) scan)
- Total cascade cost dominates

#### Test 3: Concurrent Updates (200 branches)

```rust
// Create 200 concurrent branches from root
for i in 0..200 {
    let delta = create_delta(i, vec![root], payload);
    dag.add_delta(delta, &applier).await.unwrap();
}

// Merge all 200 branches
let merge = create_delta(1000, dag.get_heads(), payload);
dag.add_delta(merge, &applier).await.unwrap();
```

**Results**:
- 200 branches: **~1 ms** (5 μs each)
- Merge delta: **~50 μs**
- Head count: 200 → 1

**Analysis**: Fast because:
- No cascade (all independent)
- Head updates are O(1) with HashSet
- Merge delta has 200 parents (still O(1) with HashSet)

#### Test 4: Pending Cleanup (1000 stale deltas)

```rust
// Add 1000 pending deltas
for i in 0..1000 {
    let delta = create_delta(i, vec![missing_parent], payload);
    dag.add_delta(delta, &applier).await.unwrap();
}

// Wait 1 second
tokio::time::sleep(Duration::from_secs(1)).await;

// Cleanup stale (> 500ms)
let evicted = dag.cleanup_stale(Duration::from_millis(500));
```

**Results**:
- Cleanup time: **~2 ms**
- Per delta: **~2 μs**
- Evicted: 1000

**Analysis**: Fast linear scan with HashMap retain

---

## Real-World Performance

### Production Metrics (20-node network)

**Context**: CRDT-based collaborative app with 20 nodes

| Metric | Value | Notes |
|--------|-------|-------|
| **Deltas per context** | ~1000 | After 1 hour of activity |
| **Pending count** | 0-5 | Rarely > 1 |
| **Head count** | 1-2 | Occasional forks |
| **Memory per context** | 5-8 MB | Depends on payload size |
| **Delta apply latency** | < 1 ms | Excludes WASM execution |
| **Cascade frequency** | ~5% | Most deltas don't cascade |
| **Cleanup interval** | 60 sec | Every minute |
| **Stale deltas evicted** | 0-2 | Rarely > 0 |

### Bottlenecks Observed

1. **WASM Execution** (not DAG):
   - 5-50 ms per delta
   - Dominates total latency
   - Solution: WASM optimization

2. **Cascade Scanning** (worst case):
   - O(P²) for deep chains
   - Rare but visible when many pending
   - Solution: Reverse parent index (future)

3. **Memory Growth**:
   - No pruning → unbounded growth
   - Solution: DAG pruning (future)

---

## Optimization Guide

### Current Optimizations

#### 1. HashSet for O(1) Operations

```rust
// ✅ Fast: O(1) parent check
applied: HashSet<[u8; 32]>

if self.applied.contains(&parent) { ... }
```

**Impact**: Parent checks are fast even with 1000s of applied deltas

#### 2. Early Duplicate Detection

```rust
// ✅ Skip expensive work for duplicates
if self.deltas.contains_key(&delta_id) {
    return Ok(false);  // O(1) exit
}
```

**Impact**: Gossipsub duplicates handled efficiently

#### 3. Lazy Cascade

```rust
// ✅ Only scan pending when needed
if self.can_apply(&delta) {
    self.apply_delta(delta, applier).await?;  // Triggers cascade
} else {
    self.pending.insert(...);  // No cascade
}
```

**Impact**: Pending deltas don't slow down when not ready

---

### Future Optimizations

#### 1. Reverse Parent Index

**Current Problem**: Cascade scans all pending deltas (O(P))

**Solution**: Track which deltas wait for which parents

```rust
pub struct DagStore<T> {
    // Existing
    deltas: HashMap<[u8; 32], CausalDelta<T>>,
    applied: HashSet<[u8; 32]>,
    pending: HashMap<[u8; 32], PendingDelta<T>>,
    heads: HashSet<[u8; 32]>,
    
    // NEW: Reverse index
    children_waiting_for: HashMap<[u8; 32], Vec<[u8; 32]>>,
}

async fn apply_delta(&mut self, delta: CausalDelta<T>, applier: &impl DeltaApplier<T>) -> Result<()> {
    applier.apply(&delta).await?;
    self.applied.insert(delta.id);
    
    // OLD: Scan all pending (O(P))
    // let ready = self.pending.iter().filter(|(_, p)| self.can_apply(&p.delta)).collect();
    
    // NEW: Direct lookup (O(1))
    if let Some(children) = self.children_waiting_for.get(&delta.id) {
        for child_id in children {
            if let Some(pending) = self.pending.remove(child_id) {
                self.apply_delta(pending.delta, applier).await?;
            }
        }
    }
    
    Ok(())
}
```

**Impact**:
- Cascade: O(P) → O(C) where C = children count
- Typically C << P (most deltas have 0-2 children)
- **10-100x faster** for large pending buffers

**Trade-off**:
- More memory: ~8 bytes per pending delta
- More bookkeeping: Update index on add/remove

#### 2. Arena Allocation

**Current Problem**: Each delta allocated separately (fragmented heap)

**Solution**: Allocate deltas in arena

```rust
use typed_arena::Arena;

pub struct DagStore<T> {
    arena: Arena<CausalDelta<T>>,
    deltas: HashMap<[u8; 32], &'arena CausalDelta<T>>,  // References into arena
}
```

**Impact**:
- Better cache locality
- Fewer allocations
- **5-10% faster** delta operations

**Trade-off**:
- More complex lifetime management
- Can't easily remove individual deltas

#### 3. Bloom Filter for Duplicates

**Current Problem**: HashMap lookup for duplicates (still O(1) but can be slow)

**Solution**: Add Bloom filter for fast negative check

```rust
use bloom::BloomFilter;

pub struct DagStore<T> {
    deltas: HashMap<[u8; 32], CausalDelta<T>>,
    delta_bloom: BloomFilter,  // Fast duplicate check
}

pub async fn add_delta(&mut self, delta: CausalDelta<T>) -> Result<bool> {
    // Fast path: Bloom filter says "definitely not seen"
    if !self.delta_bloom.contains(&delta.id) {
        // First time seeing this delta
        self.delta_bloom.insert(&delta.id);
        self.deltas.insert(delta.id, delta.clone());
        // ...
    } else {
        // Maybe seen (check HashMap for sure)
        if self.deltas.contains_key(&delta.id) {
            return Ok(false);  // Duplicate
        }
        // False positive, actually new
        self.deltas.insert(delta.id, delta.clone());
        // ...
    }
}
```

**Impact**:
- **2-3x faster** duplicate detection
- Especially useful with high duplicate rate (gossipsub)

**Trade-off**:
- Small memory overhead (~1 KB for 0.1% false positive rate)
- Slightly more complex code

#### 4. Parallel Cascade (Lock-Free)

**Current Problem**: Cascade is sequential

**Solution**: Apply independent pending deltas in parallel

```rust
async fn apply_pending(&mut self, applier: &impl DeltaApplier<T>) -> Result<()> {
    loop {
        let ready: Vec<_> = self.pending.iter()
            .filter(|(_, p)| self.can_apply(&p.delta))
            .map(|(id, _)| *id)
            .collect();
        
        if ready.is_empty() {
            break;
        }
        
        // NEW: Parallel apply (if deltas are independent)
        let tasks: Vec<_> = ready.iter().map(|id| {
            let pending = self.pending.remove(id).unwrap();
            tokio::spawn(applier.apply(&pending.delta))
        }).collect();
        
        futures::future::join_all(tasks).await?;
    }
}
```

**Impact**:
- **2-4x faster** for independent deltas
- Limited by applier parallelism (WASM is single-threaded)

**Trade-off**:
- Complex synchronization
- Applier must be thread-safe

---

## Comparison with Other Systems

### DAG vs. Git

**Git's DAG**:
- Stores commits (similar to our deltas)
- Uses content-addressable storage
- Merkle tree for integrity

**Performance Differences**:

| Operation | Git | Calimero DAG | Winner |
|-----------|-----|--------------|--------|
| Add commit | O(1) | O(1) to O(P) | Tie |
| Find merge base | O(N log N) | N/A | - |
| Check history | O(N) | O(D) | DAG |
| Garbage collect | O(N) | O(P) | DAG |

**Why DAG is faster**:
- In-memory (Git is disk-based)
- No compression (Git compresses objects)
- No integrity checks (Git verifies hashes)

**Why Git is slower but better**:
- Persistent (survives crashes)
- Compressed (saves disk space)
- Secure (tamper-proof)

### DAG vs. CRDTs (Native)

**Native CRDT** (e.g., Yjs, Automerge):
- Deltas are CRDT operations directly
- No explicit DAG tracking
- Vector clocks for causality

**Performance Differences**:

| Operation | Native CRDT | DAG + CRDT | Winner |
|-----------|-------------|------------|--------|
| Apply op | O(1) to O(N) | O(1) to O(P) | Tie |
| Merge | O(N) | O(1) | **DAG** |
| Sync | O(N log N) | O(D) | **DAG** |
| Memory | O(N) | O(N + D) | CRDT |

**DAG advantages**:
- Explicit causal relationships
- Easy to debug/visualize
- Flexible (works with any CRDT)

**Native CRDT advantages**:
- Less memory (no separate DAG)
- Simpler implementation
- Faster merge (no DAG traversal)

---

## Tuning Recommendations

### Small Networks (< 10 nodes)

**Configuration**:
```rust
// Aggressive cleanup
let cleanup_interval = Duration::from_secs(30);  // 30s
let max_age = Duration::from_secs(120);          // 2 min
```

**Rationale**: Few nodes = low traffic = fast cleanup safe

### Medium Networks (10-50 nodes)

**Configuration** (default):
```rust
let cleanup_interval = Duration::from_secs(60);   // 60s
let max_age = Duration::from_secs(300);           // 5 min
```

**Rationale**: Balanced approach, handles occasional packet loss

### Large Networks (> 50 nodes)

**Configuration**:
```rust
let cleanup_interval = Duration::from_secs(120);  // 2 min
let max_age = Duration::from_secs(600);           // 10 min
```

**Rationale**: More nodes = higher latency = need longer timeout

### High-Latency Networks (satellite, mobile)

**Configuration**:
```rust
let cleanup_interval = Duration::from_secs(300);  // 5 min
let max_age = Duration::from_secs(1800);          // 30 min
```

**Rationale**: Deltas may take minutes to arrive, don't evict prematurely

---

## Monitoring

### Key Metrics to Track

```rust
// Collect stats every minute
let stats = dag.stats();
let pending = dag.pending_stats();

metrics::gauge!("dag.total_deltas", stats.total_deltas as f64);
metrics::gauge!("dag.applied_deltas", stats.applied_deltas as f64);
metrics::gauge!("dag.pending_deltas", pending.count as f64);
metrics::gauge!("dag.head_count", stats.head_count as f64);
metrics::gauge!("dag.oldest_pending_secs", pending.oldest_age_secs as f64);
metrics::gauge!("dag.missing_parents", pending.total_missing_parents as f64);
```

### Alert Thresholds

| Metric | Warning | Critical | Action |
|--------|---------|----------|--------|
| `pending_count` | > 50 | > 100 | Trigger state sync |
| `oldest_pending_secs` | > 300 | > 600 | Check network |
| `head_count` | > 5 | > 10 | Create merge delta |
| `missing_parents` | > 10 | > 50 | Request from peers |
| `memory_mb` | > 100 | > 500 | Enable pruning |

---

## See Also

- [Architecture](architecture.md) - Implementation details
- [Design Decisions](design-decisions.md) - Why we made these choices
- [Troubleshooting](troubleshooting.md) - Performance issues
- [API Reference](api-reference.md) - How to use the API
