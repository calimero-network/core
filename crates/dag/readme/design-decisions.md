# DAG Design Decisions

Why we built the DAG this way and what alternatives we considered.

---

## Core Principles

### 1. Pure DAG Logic (No External Dependencies)

**Decision**: DAG has no dependencies on storage, network, or WASM.

**Rationale**:
- **Testability**: Can test in isolation without mocking complex services
- **Reusability**: Same DAG logic works in different contexts (node, CLI tools, tests)
- **Simplicity**: Easier to reason about and maintain
- **Performance**: No I/O blocking inside critical path

**Alternative Considered**: Integrated DAG + Storage
```rust
// ❌ Rejected: DAG with built-in storage
struct DagStore<T> {
    db: Arc<Database>,
    // Direct database access couples DAG to storage implementation
}
```

**Why Rejected**:
- Hard to test (need real or mock database)
- Can't use DAG without storage
- Storage errors mixed with DAG logic errors
- Performance: Database calls inside DAG operations

**Our Approach**: Dependency injection via `DeltaApplier`
```rust
// ✅ Accepted: Pure DAG + Injected applier
#[async_trait]
pub trait DeltaApplier<T> {
    async fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError>;
}

// Node layer provides storage applier
struct ContextStorageApplier {
    db: Arc<Database>,
    context_client: ContextClient,
}

impl DeltaApplier<Vec<Action>> for ContextStorageApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Storage logic here, not in DAG
    }
}
```

---

### 2. Generic Over Payload Type `T`

**Decision**: DAG is generic: `DagStore<T>` where `T: Clone`

**Rationale**:
- **Flexibility**: Works with any payload (actions, bytes, custom types)
- **Type Safety**: Compile-time guarantees on payload structure
- **No Serialization**: DAG doesn't serialize/deserialize (applier's job)
- **Testing**: Use simple types in tests, complex types in production

**Alternative Considered**: Fixed payload type
```rust
// ❌ Rejected: Hard-coded to Vec<Action>
struct DagStore {
    deltas: HashMap<[u8; 32], CausalDelta<Vec<Action>>>,
}
```

**Why Rejected**:
- Couples DAG to Calimero's storage layer
- Can't reuse for other delta types
- Harder to test with simple payloads

**Our Approach**:
```rust
// ✅ Accepted: Generic payload
pub struct DagStore<T> {
    deltas: HashMap<[u8; 32], CausalDelta<T>>,
}

// Use different types for different purposes
let dag_actions: DagStore<Vec<Action>> = ...;     // Production
let dag_test: DagStore<u32> = ...;                 // Tests
let dag_bytes: DagStore<Vec<u8>> = ...;            // Raw data
```

**Trade-off**: Slightly more complex API (need to specify `T`), but worth the flexibility.

---

### 3. Memory-Only (No Built-in Persistence)

**Decision**: DAG state lives entirely in RAM. No disk persistence.

**Rationale**:
- **Performance**: No I/O overhead in critical path
- **Simplicity**: No corruption, recovery, or migration logic
- **Responsibility**: Persistence is node layer's job
- **Flexibility**: Different wrappers can persist differently

**Alternative Considered**: Built-in persistence
```rust
// ❌ Rejected: DAG with disk persistence
impl DagStore<T> {
    async fn add_delta(&mut self, delta: CausalDelta<T>) -> Result<bool> {
        // Apply to memory
        self.apply_to_memory(delta)?;
        
        // Persist to disk
        self.db.save_delta(delta)?;  // Couples to storage
    }
}
```

**Why Rejected**:
- Storage failures affect DAG operations
- Can't use DAG without database
- Slower (disk I/O on every delta)
- Complex recovery logic needed

**Our Approach**: Node layer wraps DAG
```rust
// ✅ Accepted: Node layer handles persistence
pub struct DeltaStore {
    dag: Arc<RwLock<DagStore<Vec<Action>>>>,  // In memory
    applier: Arc<ContextStorageApplier>,      // Handles disk
}

impl ContextStorageApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Persist here, not in DAG
        self.context_client.execute(...).await?;
    }
}
```

**Trade-off**: Node layer more complex, but DAG stays simple and fast.

---

### 4. Automatic Cascade (Recursive Application)

**Decision**: When a delta is applied, automatically check if pending deltas are now ready.

**Rationale**:
- **Correctness**: Ensures all applicable deltas get applied
- **Developer Experience**: Callers don't need to manually trigger cascade
- **Performance**: Apply multiple deltas in one batch

**Alternative Considered**: Manual cascade
```rust
// ❌ Rejected: Caller must trigger cascade
let applied = dag.add_delta(delta1, &applier).await?;
if applied {
    // Caller must remember to check pending!
    dag.apply_pending(&applier).await?;
}
```

**Why Rejected**:
- Easy to forget
- Bug-prone (missed cascade = stuck pending deltas)
- Duplicated logic in every caller

**Our Approach**: Automatic cascade
```rust
// ✅ Accepted: Cascade happens automatically
pub async fn add_delta(&mut self, delta: CausalDelta<T>, applier: &impl DeltaApplier<T>) -> Result<bool> {
    if self.can_apply(&delta) {
        self.apply_delta(delta, applier).await?;  // Triggers cascade internally
        Ok(true)
    } else {
        self.pending.insert(delta.id, PendingDelta::new(delta));
        Ok(false)
    }
}

async fn apply_delta(&mut self, delta: CausalDelta<T>, applier: &impl DeltaApplier<T>) -> Result<()> {
    applier.apply(&delta).await?;
    self.applied.insert(delta.id);
    // ... update heads ...
    
    self.apply_pending(applier).await?;  // Automatic!
    Ok(())
}
```

**Trade-off**: Slightly slower (always checks pending), but safer and easier to use.

---

### 5. Silently Skip Duplicates (Not Error)

**Decision**: `add_delta` returns `Ok(false)` for duplicates, not an error.

**Evolution**:
```rust
// Old: Duplicates were errors
pub async fn add_delta(...) -> Result<bool, DagError> {
    if self.deltas.contains_key(&delta_id) {
        return Err(DagError::DuplicateDelta(delta_id));  // ❌
    }
}

// New: Duplicates silently skipped
pub async fn add_delta(...) -> Result<bool, DagError> {
    if self.deltas.contains_key(&delta_id) {
        return Ok(false);  // ✅ Silently skip
    }
}
```

**Rationale**:
- **Common in distributed systems**: Gossipsub duplicates messages
- **Not an error**: Duplicate delivery is expected behavior
- **Simpler error handling**: Callers don't need to handle `DuplicateDelta`

**Alternative Considered**: Return error
```rust
// ❌ Rejected: Treat duplicates as errors
match dag.add_delta(delta, &applier).await {
    Ok(applied) => { ... },
    Err(DagError::DuplicateDelta(_)) => { /* Ignore */ },  // Every caller needs this
    Err(e) => return Err(e),
}
```

**Why Rejected**:
- Noisy error logs
- Every caller must handle `DuplicateDelta`
- Not semantically an error

**Our Approach**:
```rust
// ✅ Accepted: Duplicates return Ok(false)
let applied = dag.add_delta(delta, &applier).await?;
if applied {
    // Delta was new and applied
} else {
    // Delta was duplicate or pending (both OK)
}
```

**Trade-off**: Can't distinguish "duplicate" from "pending" in return value, but that's fine.

---

### 6. HashSet for Applied Tracking

**Decision**: Use `HashSet<[u8; 32]>` for `applied` set, not `HashMap<[u8; 32], ()>`.

**Rationale**:
- **Clarity**: HashSet clearly communicates "set of IDs"
- **Memory**: Slightly less overhead than HashMap
- **API**: `contains()` more idiomatic than `contains_key()`

**Alternative Considered**: HashMap
```rust
// ❌ Rejected: HashMap with unit value
applied: HashMap<[u8; 32], ()>

if self.applied.contains_key(&parent) { ... }
```

**Why Rejected**:
- `()` value wastes space
- Less clear intent (why a map with no values?)

**Our Approach**:
```rust
// ✅ Accepted: HashSet
applied: HashSet<[u8; 32]>

if self.applied.contains(&parent) { ... }
```

**Trade-off**: None, HashSet is strictly better for this use case.

---

### 7. Heads as HashSet (Not Vec)

**Decision**: `heads: HashSet<[u8; 32]>` instead of `Vec<[u8; 32]>`

**Rationale**:
- **O(1) insert/remove**: Updating heads is fast
- **Automatic deduplication**: Can't have duplicate heads
- **Unordered**: Head order doesn't matter

**Alternative Considered**: Vec
```rust
// ❌ Rejected: Vec for heads
heads: Vec<[u8; 32]>

// Remove parent from heads (O(N) scan)
if let Some(pos) = self.heads.iter().position(|h| h == parent) {
    self.heads.remove(pos);
}
```

**Why Rejected**:
- O(N) to remove (slow for many heads)
- Duplicates possible (need manual checks)

**Our Approach**:
```rust
// ✅ Accepted: HashSet
heads: HashSet<[u8; 32]>

// O(1) remove
self.heads.remove(parent);

// O(1) insert (auto-dedups)
self.heads.insert(delta.id);
```

**Trade-off**: Can't maintain head order, but order doesn't matter for correctness.

---

### 8. Pending Cleanup by Timeout (Not Count Limit)

**Decision**: Clean up pending deltas by age, not by count limit.

**Rationale**:
- **Fairness**: Old deltas evicted first (FIFO)
- **Predictability**: Timeout is deterministic
- **Flexibility**: Node can set different timeouts per environment

**Alternative Considered**: Count limit
```rust
// ❌ Rejected: Fixed count limit
const MAX_PENDING: usize = 100;

if self.pending.len() > MAX_PENDING {
    // Which to evict? Arbitrary choice.
    let to_remove = self.pending.len() - MAX_PENDING;
    // ...
}
```

**Why Rejected**:
- Arbitrary limit (what if network slow?)
- Doesn't account for pending delta age
- Might evict deltas about to be unlocked

**Our Approach**:
```rust
// ✅ Accepted: Timeout-based eviction
pub fn cleanup_stale(&mut self, max_age: Duration) -> usize {
    self.pending.retain(|_id, pending| pending.age() <= max_age);
}

// Node layer calls with 5-minute timeout
dag.cleanup_stale(Duration::from_secs(300));
```

**Trade-off**: Unbounded pending count if timeout too long, but caller can choose.

---

### 9. Sync via get_deltas_since (Not Full State)

**Decision**: Sync by transferring missing deltas, not full state snapshots.

**Rationale**:
- **Efficient**: Only send deltas since common ancestor
- **Incremental**: Works for catch-up and periodic sync
- **CRDT-friendly**: Deltas merge automatically

**Alternative Considered**: Full state sync
```rust
// ❌ Rejected: Send entire state
pub fn get_full_state(&self) -> HashMap<Key, Value> {
    self.storage.clone()  // Expensive!
}
```

**Why Rejected**:
- Expensive for large state (MB to GB)
- Wasteful if already mostly in sync
- Requires state serialization format

**Our Approach**:
```rust
// ✅ Accepted: Delta sync
pub fn get_deltas_since(
    &self,
    ancestor: [u8; 32],
    start_id: Option<[u8; 32]>,
    query_limit: usize,
) -> Vec<CausalDelta<T>> {
    // BFS from heads to ancestor
    // Returns only missing deltas
}
```

**Trade-off**: Requires finding common ancestor (handled by sync protocol).

---

### 10. No Automatic Merge Creation

**Decision**: DAG detects forks (multiple heads) but doesn't auto-create merge deltas.

**Rationale**:
- **Separation of concerns**: DAG is pure logic, merge is policy
- **Flexibility**: Node layer decides when/how to merge
- **Testing**: Easier to test fork scenarios without automatic resolution

**Alternative Considered**: Auto-merge
```rust
// ❌ Rejected: DAG creates merge deltas
impl DagStore<T> {
    async fn add_delta(&mut self, delta: CausalDelta<T>) -> Result<bool> {
        self.apply_delta(delta, applier).await?;
        
        // Auto-merge if fork detected
        if self.heads.len() > 1 {
            let merge = self.create_merge_delta();
            self.add_delta(merge, applier).await?;
        }
    }
}
```

**Why Rejected**:
- DAG needs to know how to create valid payloads (violates pure logic)
- Different merge strategies for different applications
- Can create infinite merge loops

**Our Approach**:
```rust
// ✅ Accepted: DAG reports forks, node layer decides
let heads = dag.get_heads();
if heads.len() > 1 {
    // Node layer decides merge strategy
    let merge_delta = create_merge_according_to_policy(heads);
    dag.add_delta(merge_delta, &applier).await?;
}
```

**Trade-off**: Node layer must implement merge logic, but more flexible.

---

### 11. Clone Requirement on Payload

**Decision**: Payload must implement `Clone`: `DagStore<T> where T: Clone`

**Rationale**:
- **Simplicity**: Can clone deltas when needed (e.g., `get_deltas_since`)
- **Common**: Most delta payloads are already Clone
- **Performance**: Usually cheap (Arc for large data)

**Alternative Considered**: No Clone requirement
```rust
// ❌ Rejected: No Clone bound
pub struct DagStore<T> {  // No bounds
    deltas: HashMap<[u8; 32], CausalDelta<T>>,
}

// Can't return Vec<CausalDelta<T>>
pub fn get_deltas_since(
    &self,
    ancestor: [u8; 32],
    start_id: Option<[u8; 32]>,
    query_limit: usize,
) -> Vec<CausalDelta<T>> {
    // Returns references instead
}
```

**Why Rejected**:
- Lifetime complications (references tied to DAG lifetime)
- Can't return owned deltas
- Harder to use in async contexts

**Our Approach**:
```rust
// ✅ Accepted: Require Clone
impl<T: Clone> DagStore<T> {
    pub fn get_deltas_since(
        &self,
        ancestor: [u8; 32],
        start_id: Option<[u8; 32]>,
        query_limit: usize,
    ) -> Vec<CausalDelta<T>> {
        // Can return owned copies
        result.push(delta.clone());
    }
}
```

**Trade-off**: Payloads must be Clone, but this is reasonable for CRDTs.

---

### 12. Async Applier (Not Sync)

**Decision**: `DeltaApplier::apply` is async.

**Rationale**:
- **WASM execution is async**: Context client uses async executor
- **Storage I/O is async**: Database writes are async
- **Future-proof**: Can add async operations later

**Alternative Considered**: Sync applier
```rust
// ❌ Rejected: Sync applier
pub trait DeltaApplier<T> {
    fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError>;
}
```

**Why Rejected**:
- Can't call async functions from sync trait
- Blocks executor thread (bad performance)
- Node layer needs async anyway

**Our Approach**:
```rust
// ✅ Accepted: Async applier
#[async_trait]
pub trait DeltaApplier<T> {
    async fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError>;
}
```

**Trade-off**: Requires async runtime, but we already have one (tokio).

---

## Comparison with Alternatives

### DAG vs. Vector Clocks

**Vector Clocks**: Each node tracks version numbers for all nodes

```
Node A: {A: 5, B: 3, C: 2}
Node B: {A: 4, B: 7, C: 2}

Merge: {A: max(5,4), B: max(3,7), C: max(2,2)} = {A: 5, B: 7, C: 2}
```

**Why we chose DAG**:
- ✅ Explicit causal relationships (parent pointers)
- ✅ Works with partial state (don't need full history)
- ✅ Natural fit for CRDTs (deltas are CRDT operations)
- ✅ Easier to debug (can visualize DAG)

**Vector Clock advantages** we gave up:
- ❌ Smaller metadata (one integer per node vs. 32-byte hashes)
- ❌ Simpler comparison (element-wise max vs. DAG traversal)

**Decision**: DAG's explicitness outweighs vector clock's compactness.

### DAG vs. Merkle Trees

**Merkle Trees**: Hash-based tree structure

```
         Root
        /    \
      H1      H2
     /  \    /  \
   L1  L2  L3  L4
```

**Why we chose DAG**:
- ✅ Supports branching (forks)
- ✅ Multiple heads (concurrent updates)
- ✅ Flexible topology (not strictly tree)

**Merkle Tree advantages** we gave up:
- ❌ Efficient diff (compare root hash)
- ❌ Compact proofs (log N proof size)

**Decision**: DAG's flexibility for concurrent updates is critical.

### DAG vs. Event Sourcing

**Event Sourcing**: Append-only log of events

```
Event1 → Event2 → Event3 → Event4 → ...
```

**Why we chose DAG**:
- ✅ Handles concurrent updates (event sourcing is linear)
- ✅ Merges parallel histories (event sourcing has conflicts)
- ✅ Distributed-first (event sourcing is single-writer)

**Event Sourcing advantages** we gave up:
- ❌ Simpler (just append)
- ❌ Guaranteed order (no out-of-order issues)

**Decision**: Distributed collaboration requires DAG's branching.

---

## Future Considerations

### Reverse Parent Index

**Current**: Cascade scans all pending deltas (O(P))

**Future**: Maintain index `parent_id → [child_ids]`

```rust
pub struct DagStore<T> {
    // Existing
    deltas: HashMap<[u8; 32], CausalDelta<T>>,
    applied: HashSet<[u8; 32]>,
    pending: HashMap<[u8; 32], PendingDelta<T>>,
    heads: HashSet<[u8; 32]>,
    
    // New: Reverse index
    children_waiting_for: HashMap<[u8; 32], Vec<[u8; 32]>>,
}
```

**Benefits**:
- Cascade: O(P) → O(children)
- Typically children << P

**Trade-off**: More memory, more complex bookkeeping

### DAG Pruning

**Current**: All deltas kept in memory forever

**Future**: Prune old deltas beyond snapshot threshold

```rust
pub fn prune_before(&mut self, snapshot: [u8; 32]) {
    // Remove all deltas before snapshot
    // Keep only deltas after (for sync)
}
```

**Benefits**:
- Bounded memory growth
- Support long-running nodes

**Trade-off**: Can't sync from genesis (need snapshot protocol)

### Batched Operations

**Current**: One delta at a time

**Future**: Add multiple deltas in one batch

```rust
pub async fn add_deltas_batch(
    &mut self,
    deltas: Vec<CausalDelta<T>>,
    applier: &impl DeltaApplier<T>,
) -> Result<Vec<bool>> {
    // Apply all in topological order
    // Single cascade check at end
}
```

**Benefits**:
- More efficient (one cascade check)
- Better for sync (apply many deltas)

**Trade-off**: More complex API

---

## Lessons Learned

### What Worked Well

1. **Dependency injection**: Makes testing easy
2. **Generics**: Flexible payload types
3. **Memory-only**: Fast and simple
4. **Automatic cascade**: Saves caller complexity

### What We'd Change

1. **Add reverse index sooner**: Cascade can be slow
2. **Better duplicate handling**: Return value ambiguous (duplicate vs pending)
3. **Pruning from start**: Memory growth issue discovered late

---

## See Also

- [Architecture](architecture.md) - How it's implemented
- [API Reference](api-reference.md) - How to use it
- [Performance](performance.md) - How fast it is
- [Main README](../README.md) - What it does
