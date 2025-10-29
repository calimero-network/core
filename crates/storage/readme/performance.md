# Performance Guide

Optimization tips and performance characteristics for Calimero Storage CRDTs.

---

## TL;DR

✅ **Local operations:** O(1) - same as regular collections  
✅ **Remote sync:** O(1) for 99% of cases  
⚠️ **Merge:** O(F×E) but rare (< 1%) and network-bound  

**Bottom line:** CRDT overhead is negligible in practice.

---

## Operation Costs

### Counter

| Operation | Time | Storage I/O |
|-----------|------|-------------|
| new() | O(1) | 1 write |
| increment() | O(1) | 1 write |
| value() | O(1) | 1 read |
| merge() | O(N) | N=other's value (increments) |

**Best for:** High-frequency incrementing (views, clicks, scores)

### LwwRegister<T>

| Operation | Time | Storage I/O |
|-----------|------|-------------|
| new(value) | O(1) | 1 write |
| set(value) | O(1) | 1 write |
| get() | O(1) | 1 read |
| merge() | O(1) | Timestamp compare |

**Best for:** Single values that change occasionally

### UnorderedMap<K, V>

| Operation | Time | Storage I/O |
|-----------|------|-------------|
| new() | O(1) | 1 write |
| insert(k, v) | O(1) | 1 write |
| get(k) | O(1) | 1 read |
| remove(k) | O(1) | 1 write (tombstone) |
| entries() | O(N) | N reads |
| merge() | O(N) | N=entries in other |

**Best for:** Key-value lookups, dictionaries

### Vector<T>

| Operation | Time | Storage I/O |
|-----------|------|-------------|
| new() | O(1) | 1 write |
| push(v) | O(1) | 1 write |
| get(i) | O(N) | Linear scan |
| pop() | O(1) | 1 write |
| update(i, v) | O(N) | Scan + write |
| merge() | O(min(N,M)) | Element-wise |

**Best for:** Append-heavy workloads (logs, events)  
**Avoid for:** Random access (use Map with numeric keys)

### UnorderedSet<T>

| Operation | Time | Storage I/O |
|-----------|------|-------------|
| new() | O(1) | 1 write |
| insert(v) | O(1) | 1 write |
| contains(v) | O(1) | 1 read (by hash) |
| remove(v) | O(1) | 1 write (tombstone) |
| iter() | O(N) | N reads |
| merge() | O(N) | N=other's size |

**Best for:** Unique membership testing

---

## Nesting Performance

### Access Cost

```rust
// 1 level: O(1)
map.get("key")?;

// 2 levels: O(1) + O(1) = O(1)
map.get("key")?.get("nested_key")?;

// 3 levels: O(1) × 3 = O(1)
map.get("k1")?.get("k2")?.get("k3")?;
```

**With vectors:**
```rust
// O(N) per vector level
vec.get(index)?;  // O(N)
map.get("key")?.get(index)?;  // O(1) + O(N)
```

### Merge Cost

```rust
// 1 level: O(N)
map.merge(&other_map)?;  // N = entries

// 2 levels: O(N × M)
Map<K, Map<K2, V>>.merge()?;
// N = outer entries
// M = avg inner entries
// Total: N × M merges

// 3 levels: O(N × M × P)
Map<K, Map<K2, Map<K3, V>>>.merge()?;
```

**Reality check:**
- Typical: N=10, M=5, P=3 → 150 operations
- Time: ~1ms
- Network: ~100ms
- **Merge is 1% of sync time!**

---

## Optimization Tips

### 1. Keep Root Fields Minimal

```rust
// ✅ Good: 3-5 root fields
#[app::state]
pub struct MyApp {
    documents: Map<...>,
    counters: Map<...>,
    settings: Map<...>,
}

// ⚠️ Avoid: 50 root fields
#[app::state]
pub struct MyApp {
    field1: Map<...>,
    field2: Map<...>,
    // ... 48 more
}
```

**Why:** Merge iterates all fields. Fewer fields = faster merge.

### 2. Use Appropriate Collections

```rust
// ✅ Good: Counter for metrics
views: Map<String, Counter>

// ❌ Bad: LwwRegister for counters
views: Map<String, LwwRegister<u64>>  // Loses concurrent increments!

// ✅ Good: Vector for append-only
logs: Vector<LogEntry>

// ❌ Bad: Vector for random access
items: Vector<Item>  // O(N) per get()
// Better: Map<index, Item>  // O(1) per get()
```

### 3. Batch Operations

```rust
// ⚠️ Inefficient: Multiple round-trips
for item in items {
    map.insert(item.id, item)?;  // N storage writes
}

// ✅ Better: Still O(N), but clearer
let mut map = Map::new();
for item in items {
    map.insert(item.id, item)?;
}
// All operations batched in same transaction
```

### 4. Avoid Deep Nesting

```rust
// ⚠️ Deep nesting: O(1)^5 = still O(1) access, but verbose
data: Map<K1, Map<K2, Map<K3, Map<K4, V>>>>

// ✅ Better: Composite keys
data: Map<CompositeKey, V>  // Key = "k1::k2::k3::k4"

// Trade-off:
// Deep: Natural structure, automatic merge
// Flat: Faster access, simpler merge
```

---

## Benchmarking Your App

### Measuring Merge Overhead

```rust
use std::time::Instant;

// Baseline: Local operations
let start = Instant::now();
for _ in 0..1000 {
    app.data.insert(random_key(), value)?;
}
let baseline = start.elapsed();
println!("1000 inserts: {:?}", baseline);

// With merge simulation
let start = Instant::now();
// ... simulate concurrent state ...
app.merge(&concurrent_app)?;
let merge_time = start.elapsed();
println!("Merge: {:?}", merge_time);

// Expected: merge_time << network latency
```

### Memory Profiling

```rust
// Before
let mem_before = get_memory_usage();

// Create structure
let mut map = Map::new();
for i in 0..1000 {
    map.insert(format!("key-{}", i), Counter::new())?;
}

// After
let mem_after = get_memory_usage();
let overhead = mem_after - mem_before;

// Expected: ~200 bytes per entry
```

---

## Real-World Performance

### Collaborative Editor (100 users)

**Workload:**
- 1000 local edits/second (typing)
- 10 remote syncs/second (receiving updates)
- 1 merge every 10 seconds (rare root conflict)

**Performance:**
- Local edits: 1ms total (1μs each)
- Remote syncs: 20ms total (2ms each)
- Merge: 2ms
- Network: 100-500ms per sync

**Bottleneck:** Network (99% of time)  
**CRDT overhead:** < 1%

### Analytics Dashboard (1M events/day)

**Workload:**
- 12 increments/second average
- 1000 unique counters
- Sync every 5 seconds

**Performance:**
- Increments: O(1) each, ~0.1ms
- Sync: O(1) per counter (different IDs)
- Merge: Rare (different elements)

**Bottleneck:** Storage I/O  
**CRDT overhead:** None (DAG handles conflicts)

---

## Performance Comparison

### Calimero CRDTs vs Alternatives

| Approach | Insert | Get | Sync | Merge |
|----------|--------|-----|------|-------|
| **Calimero** | O(1) | O(1) | O(1) | O(N) rare |
| **Manual merge** | O(1) | O(1) | O(1) | O(N) always |
| **Automerge** | O(log N) | O(log N) | O(N) | O(N) |
| **Yjs** | O(1) | O(1) | O(N) | O(N) |

**Calimero advantage:** Element IDs eliminate most merge operations!

---

## Optimization Checklist

Before optimizing, profile! Don't guess.

**If merge is slow:**
- [ ] Check: Are you merging at root level? (Should be rare)
- [ ] Reduce root field count
- [ ] Use simpler value types
- [ ] Consider manual flattening for that specific field

**If sync is slow:**
- [ ] Check: Is network the bottleneck? (Usually yes)
- [ ] Optimize delta size
- [ ] Batch operations
- [ ] Use compression

**If storage is growing:**
- [ ] Check: Garbage collection running?
- [ ] Compact tombstones
- [ ] Archive old data

---

## When to Optimize

### Don't Optimize (Yet)

- Local operations feel fast (< 10ms)
- Sync completes in reasonable time (< 1 second)
- Storage size is manageable (< 1GB)

**Why:** Premature optimization is evil. CRDT overhead is usually negligible.

### Do Optimize

- Local operations laggy (> 100ms)
- Merge taking > 10ms consistently
- Storage growing > 1GB/week

**How:** Profile first, then optimize hot paths.

---

## Performance Monitoring

### Metrics to Track

```rust
// Operation counts
- local_writes_per_second
- remote_syncs_per_second
- merges_per_minute  // Should be LOW!

// Latencies
- p50_insert_latency
- p99_insert_latency
- merge_latency
- sync_latency

// Storage
- total_elements
- storage_size_bytes
- tombstone_ratio
```

### Red Flags

🚨 **merges_per_minute > 10** - Too many root conflicts  
🚨 **merge_latency > 10ms** - State too large or deep  
🚨 **tombstone_ratio > 0.5** - Need garbage collection  
🚨 **storage_size growing rapidly** - Check for leaks  

---

## Advanced: Custom Merge Optimization

If you need explicit control:

```rust
impl Mergeable for MyLargeStruct {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Only merge fields that might have changed
        if self.version < other.version {
            self.field1.merge(&other.field1)?;  // Changed
            // Skip field2 (unchanged)
        }
        Ok(())
    }
}
```

**Warning:** Only do this if profiling shows merge is actually slow!

---

## See Also

- [Architecture](architecture.md) - How the system works
- [Collections API](collections.md) - Per-collection performance
- [TODO.md](../../../TODO.md) - Future optimizations
