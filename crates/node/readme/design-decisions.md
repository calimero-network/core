# Node Design Decisions

Why we built the node layer this way and what alternatives we considered.

---

## Core Principles

### 1. Dual-Path Synchronization

**Decision**: Use both Gossipsub (primary) and P2P sync (fallback)

**Rationale**:
- **Gossipsub alone**: Fast but unreliable (packet loss, partitions)
- **P2P alone**: Reliable but slow (polling overhead)
- **Both**: Best of both worlds

**Alternative Considered**: Gossipsub only

**Why Rejected**:
- Packet loss can cause permanent divergence
- Network partitions leave nodes out of sync
- No recovery mechanism

**Trade-off**: More complex, but essential for eventual consistency

---

### 2. Author Nodes Skip Handlers

**Decision**: Nodes that create deltas don't execute their own event handlers

**Rationale**:
- **Prevents infinite loops**: Handler → Event → Handler → ...
- **Distributed execution model**: Only receivers react
- **Clearer semantics**: Author did the work, receivers observe

**Alternative Considered**: Authors execute handlers too

**Why Rejected**:
```rust
// Would cause infinite loop:
pub fn add_item(&mut self, name: String) {
    self.items.insert(name.clone(), Item::new());
    app::emit!(ItemAdded { name });
}

#[app::event_handler]
pub fn on_item_added(&mut self, event: ItemAdded) {
    // If author executes this, it would emit another event:
    app::emit!(ItemAddedNotification { ... });
    // → Infinite recursion!
}
```

**Our Approach**: Author skipped, clean separation

---

### 3. Periodic Cleanup Timers

**Decision**: Run cleanup every 60 seconds (deltas) and 5 minutes (blobs)

**Rationale**:
- **Bounded memory**: Prevents unbounded growth
- **Early detection**: Spot stuck deltas quickly
- **Automatic recovery**: No manual intervention needed

**Alternative Considered**: Manual cleanup only

**Why Rejected**:
- Easy to forget
- Memory leaks in long-running nodes
- No early warning of issues

**Frequencies Chosen**:
- 60s deltas: Balance between overhead and detection speed
- 5min blobs: LRU cache naturally stable, less frequent cleanup OK

---

### 4. Blob LRU Cache (3-Phase Eviction)

**Decision**: Use 3-phase eviction (age, count, size)

**Rationale**:
- **Phase 1 (age)**: Quick removal of stale blobs (70% evicted)
- **Phase 2 (count)**: Prevent too many small blobs (20% evicted)
- **Phase 3 (size)**: Hard memory limit (10% evicted)

**Alternative Considered**: Single-phase (LRU only)

**Why Rejected**:
- Age-based removes stale quickly
- Count-based prevents blob spam
- Size-based prevents OOM

**Example**: Without age-based, 1000 small blobs (100 KB each) would take forever to evict via LRU

---

### 5. Hash Heartbeat Broadcasting

**Decision**: Broadcast root hash + DAG heads every 30 seconds

**Rationale**:
- **Silent divergence detection**: Catches non-deterministic CRDTs
- **Early warning**: Detect before users notice
- **Trigger recovery**: Auto-trigger state sync

**Alternative Considered**: No heartbeat (rely on sync only)

**Why Rejected**:
```
Without heartbeat:
- Node A: hash = 0xABCD
- Node B: hash = 0x1234
- Nodes never sync (heads match but state different)
- Users see different data!

With heartbeat:
- Node A broadcasts 0xABCD
- Node B compares with own 0x1234
- Mismatch detected → trigger full state sync
- Divergence resolved
```

**Frequency (30s)**: Balance detection speed vs. network overhead

---

### 6. Arbiter Pool for Task Distribution

**Decision**: Use Actix arbiter pool for parallel task execution

**Rationale**:
- **CPU utilization**: Spread WASM execution across cores
- **Fairness**: Round-robin prevents starvation
- **Simple**: Built into Actix, no custom threadpool

**Alternative Considered**: Single-threaded (one arbiter)

**Why Rejected**:
- Wastes CPU cores
- WASM execution blocks other operations
- Poor throughput

**Alternative Considered**: Tokio threadpool directly

**Why Rejected**:
- Actix actors + tokio tasks = complex
- Arbiters integrate better with Actix

---

### 7. Arc<RwLock<>> for DAG Access

**Decision**: Wrap DAG in Arc<RwLock<>> for thread-safe access

**Rationale**:
- **Read parallelism**: Multiple readers simultaneously
- **Write exclusivity**: One writer at a time
- **Simple**: Standard Rust pattern

**Alternative Considered**: Mutex

**Why Rejected**:
```rust
// Mutex blocks readers during reads
let dag = self.dag.lock().await;  // Blocks everyone

// RwLock allows concurrent reads
let dag = self.dag.read().await;   // Multiple readers OK
let dag = self.dag.write().await;  // One writer
```

**Alternative Considered**: Lock-free data structures

**Why Rejected**:
- Too complex for marginal gains
- RwLock good enough for our access patterns

---

### 8. DeltaStore per Context (Not Global)

**Decision**: Each context has its own DeltaStore + DAG

**Rationale**:
- **Isolation**: Contexts don't affect each other
- **Concurrency**: Parallel operations on different contexts
- **Scaling**: Add contexts without global lock contention

**Alternative Considered**: Single global DAG

**Why Rejected**:
- Global lock = bottleneck
- One context's issues affect all
- Memory sharing not beneficial

**Memory Trade-off**: More memory (separate DAGs) but better concurrency

---

### 9. Periodic Sync (Not Event-Driven)

**Decision**: Sync every 10 seconds (timer-based)

**Rationale**:
- **Predictable**: Happens regardless of traffic
- **Simple**: No complex event triggering
- **Reliable**: Catches silent failures

**Alternative Considered**: Event-driven (sync on broadcast failure)

**Why Rejected**:
```rust
// How to detect broadcast failure?
// Gossipsub doesn't report delivery failures!

// Can't trigger sync on failure if we don't know it failed
```

**Our Approach**: Periodic sync catches failures automatically

---

### 10. Sync Tracking (Prevent Spam)

**Decision**: Track last sync time per context, enforce minimum interval

**Rationale**:
- **Prevent redundant syncs**: Same context every 10s = waste
- **Bandwidth savings**: Only sync when needed
- **Fairness**: Spread syncs across contexts

**Alternative Considered**: No tracking (sync all contexts every time)

**Why Rejected**:
```
100 contexts, 10s frequency:
  → 100 syncs every 10s
  → 10 syncs/sec continuous

With 5s interval tracking:
  → Max 100 syncs / 5s = 20 syncs/sec
  → But spread over time, avg ~10 syncs/sec
```

---

### 11. NodeManager as Actix Actor

**Decision**: Use Actix Actor for NodeManager

**Rationale**:
- **Message passing**: Clean async communication
- **Timers**: Built-in `run_interval`
- **Lifecycle**: `started()`, `stopped()` hooks
- **Supervision**: Actor restart on panic

**Alternative Considered**: Pure async (no actors)

**Why Rejected**:
- Need timers → would implement ourselves
- Need lifecycle → would implement ourselves
- Need message queue → would implement ourselves
- Actix provides all this

**Trade-off**: Actix dependency, but we already use it

---

### 12. WASM for Delta Application

**Decision**: Execute `__calimero_sync_next` in WASM to apply deltas

**Rationale**:
- **Sandboxing**: WASM is isolated
- **Determinism**: Same WASM, same result
- **Flexibility**: Apps define their own CRDTs

**Alternative Considered**: Native Rust CRDTs

**Why Rejected**:
- Not flexible (hard-coded CRDTs)
- Can't upgrade without recompiling node
- Apps can't define custom types

**Trade-off**: WASM slower than native, but flexibility worth it

---

## Comparison with Alternatives

### Node vs. Pure Gossipsub

**Pure Gossipsub**:
- Simpler (no sync protocol)
- Faster (no periodic overhead)
- Unreliable (no recovery)

**Our Approach (Dual-path)**:
- More complex
- Slightly more overhead
- Eventually consistent (reliable)

**Decision**: Reliability > Simplicity

---

### Node vs. Blockchain

**Blockchain**:
- Total ordering (all nodes same order)
- Consensus overhead (slow)
- Global state (expensive)

**Our Approach (CRDT + DAG)**:
- Partial ordering (causality only)
- No consensus (fast)
- Local state (efficient)

**Decision**: CRDT model fits collaborative apps better

---

## Lessons Learned

### What Worked Well

1. **Dual-path sync**: Catches all edge cases
2. **Author skip handlers**: Clean semantics
3. **Periodic cleanup**: Prevents memory leaks
4. **Hash heartbeat**: Catches divergence early

### What We'd Change

1. **Add reverse parent index in DAG**: Cascade too slow
2. **Make DeltaStore async-first**: RwLock contention
3. **Add metrics earlier**: Hard to debug without

---

## Future Considerations

### Potential Improvements

1. **Adaptive sync**: Adjust frequency based on traffic
2. **Batched broadcasts**: Combine multiple deltas
3. **Delta compression**: Reduce network usage
4. **State snapshots**: Full resync for large gaps

---

## See Also

- [Architecture](architecture.md) - How it's implemented
- [Sync Protocol](sync-protocol.md) - How sync works
- [Performance](performance.md) - Performance impact
- [DAG Design Decisions](../../dag/readme/design-decisions.md) - DAG-level decisions

