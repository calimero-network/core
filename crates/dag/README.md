# calimero-dag

Pure DAG (Directed Acyclic Graph) implementation for causal delta tracking with automatic fork detection and resolution.

## Overview

This crate provides a lightweight, storage-agnostic DAG implementation for managing causal relationships between deltas. It handles topology, ordering, buffering, and automatic merge detection.

## Key Features

- ✅ **Causal ordering**: Deltas applied only when all parents available
- ✅ **Out-of-order delivery**: Buffers deltas until dependencies arrive
- ✅ **Fork detection**: Tracks multiple concurrent heads
- ✅ **Automatic cascade**: Applying one delta unlocks pending children
- ✅ **Generic payload**: Works with any delta content type
- ✅ **Dependency injection**: Pluggable applier for testing

## Quick Start

```rust
use calimero_dag::{DagStore, CausalDelta, DeltaApplier};

// Define how to apply deltas
struct MyApplier;

#[async_trait::async_trait]
impl DeltaApplier<MyPayload> for MyApplier {
    async fn apply(&self, delta: &CausalDelta<MyPayload>) -> Result<(), ApplyError> {
        // Your application logic here
        Ok(())
    }
}

// Create DAG starting from root
let mut dag = DagStore::new([0; 32]);
let applier = MyApplier;

// Add delta
let delta = CausalDelta {
    id: [1; 32],
    parents: vec![[0; 32]],  // Parent: root
    payload: my_data,
    timestamp: now(),
};

let applied = dag.add_delta(delta, &applier).await?;
assert!(applied);  // true = applied immediately, false = pending
```

## Core Types

### CausalDelta<T>

A delta with parent references for causal ordering:

```rust
pub struct CausalDelta<T> {
    pub id: [u8; 32],           // Unique delta ID (content hash)
    pub parents: Vec<[u8; 32]>, // Parent delta IDs
    pub payload: T,             // The actual delta content
    pub timestamp: u64,         // Creation timestamp
}
```

### DagStore<T>

Manages DAG topology and applies deltas in causal order:

```rust
pub struct DagStore<T> {
    deltas: HashMap<[u8; 32], CausalDelta<T>>,  // All deltas seen
    applied: HashSet<[u8; 32]>,                  // Successfully applied
    pending: HashMap<[u8; 32], PendingDelta<T>>, // Waiting for parents
    heads: HashSet<[u8; 32]>,                    // Current tips
    root: [u8; 32],                              // Genesis
}
```

**Key Methods**:
- `add_delta()` - Add delta (applies if parents ready, buffers otherwise)
- `get_heads()` - Get current DAG heads (for creating new deltas)
- `get_missing_parents()` - Get parent IDs needed by pending deltas
- `cleanup_stale()` - Evict old pending deltas (timeout-based)

## How It Works

### Linear Chain (Simple Case)

```
Root → Delta1 → Delta2 → Delta3
[0;32]   [1;32]   [2;32]   [3;32]

All deltas applied immediately (parents available)
Heads: [Delta3]
```

### Out-of-Order Delivery

```
Receive: Delta2 (parents: [Delta1])
→ Delta1 not applied yet
→ Buffer Delta2 as pending
→ Heads: [Root]

Receive: Delta1 (parents: [Root])
→ Root is applied
→ Apply Delta1 immediately
→ Check pending → Delta2 now ready
→ Apply Delta2 automatically (cascade)
→ Heads: [Delta2]
```

### Concurrent Updates (Fork)

```
Initial: Heads = [Delta5]

Node A creates Delta6A (parents: [Delta5])
Node B creates Delta6B (parents: [Delta5])

Both received:
→ Both deltas applied (parent Delta5 exists)
→ Heads: [Delta6A, Delta6B]  // FORK DETECTED!

Next operation creates merge:
Delta7 (parents: [Delta6A, Delta6B])
→ Heads: [Delta7]  // Fork resolved
```

## API Examples

### Adding Deltas

```rust
// Add delta that builds on current heads
let delta = CausalDelta {
    id: compute_id(),
    parents: dag.get_heads(),  // Build on all current heads
    payload: my_changes,
    timestamp: now(),
};

match dag.add_delta(delta, &applier).await? {
    true => println!("Applied immediately"),
    false => println!("Pending (waiting for parents)"),
}
```

### Handling Missing Parents

```rust
// After adding delta that's pending
let missing = dag.get_missing_parents();

if !missing.is_empty() {
    // Request these deltas from peers
    for parent_id in missing {
        request_from_network(parent_id).await?;
    }
}
```

### Fork Detection

```rust
let heads = dag.get_heads();

match heads.len() {
    1 => println!("Linear history"),
    2.. => println!("Fork detected! {} concurrent heads", heads.len()),
    0 => unreachable!("Always at least root"),
}

// Next delta should merge all heads
let merge_delta = CausalDelta {
    id: compute_id(),
    parents: heads,  // ALL current heads
    payload: merge_operation,
    timestamp: now(),
};
```

### Cleanup

```rust
// Periodic cleanup of stale pending deltas
let max_age = Duration::from_secs(300);  // 5 minutes
let evicted = dag.cleanup_stale(max_age);

if evicted > 0 {
    warn!("Evicted {} stale pending deltas", evicted);
}
```

## Integration with calimero-node

The `DagStore` is wrapped by `DeltaStore` in `calimero-node`:

```rust
// In crates/node/src/delta_store.rs
pub struct DeltaStore {
    dag: Arc<RwLock<CoreDagStore<Vec<Action>>>>,
    applier: Arc<ContextStorageApplier>,
}

impl DeltaStore {
    pub async fn add_delta(&self, delta: CausalDelta<Vec<Action>>) -> Result<bool> {
        let mut dag = self.dag.write().await;
        let result = dag.add_delta(delta, &*self.applier).await?;
        
        // Update context's dag_heads
        let heads = dag.get_heads();
        self.applier.context_client
            .update_dag_heads(&self.applier.context_id, heads)?;
        
        Ok(result)
    }
}
```

The applier connects DAG to WASM storage:

```rust
#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for ContextStorageApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Convert actions to StorageDelta
        let artifact = borsh::to_vec(&StorageDelta::Actions(delta.payload.clone()))?;
        
        // Execute __calimero_sync_next in WASM
        let outcome = self.context_client
            .execute(&self.context_id, &self.our_identity, 
                    "__calimero_sync_next", artifact, vec![], None)
            .await?;
        
        // Storage updated, new root_hash in outcome
        Ok(())
    }
}
```

## Design Principles

### Pure DAG Logic

This crate is **intentionally minimal**:
- ✅ No network code
- ✅ No storage code
- ✅ No WASM runtime dependencies
- ✅ Generic over payload type

**Why**: Separation of concerns enables:
- Easy unit testing with mock appliers
- Reuse in different contexts (not just storage)
- Clear boundaries between topology and application

### Dependency Injection

The applier pattern allows plugging in different behaviors:

```rust
// Testing
struct MockApplier {
    applied: Arc<Mutex<Vec<[u8; 32]>>>,
}

// Production
struct WasmApplier {
    context_client: ContextClient,
    context_id: ContextId,
}

// Both implement DeltaApplier trait
```

## Testing

```bash
# Run all tests
cargo test -p calimero-dag

# Run specific test
cargo test -p calimero-dag test_dag_out_of_order

# With output
cargo test -p calimero-dag -- --nocapture
```

### Test Coverage

1. **test_dag_linear_sequence**: Simple chain Root → D1 → D2
2. **test_dag_out_of_order**: D2 arrives before D1 (buffering + cascade)
3. **test_dag_concurrent_updates**: Fork detection and multiple heads
4. **test_dag_cleanup_stale**: Timeout-based eviction

## Performance Characteristics

### Memory Usage

Per delta: ~200 bytes overhead + payload size

For 1000 deltas with 5KB payloads:
```
deltas HashMap:  1000 × 5KB = 5 MB
applied HashSet: 1000 × 32B = 32 KB
pending HashMap: variable (0-1000 deltas)
heads HashSet:   ~1-10 × 32B = 32-320 bytes

Total: ~5 MB + pending
```

### Time Complexity

- `add_delta`: O(1) if applied immediately, O(P) if pending (P = pending count to check)
- `get_heads`: O(H) where H = head count (typically 1-10)
- `get_missing_parents`: O(P × M) where P = pending, M = avg parents per delta
- `cleanup_stale`: O(P) where P = pending count

### Cascade Performance

When adding a delta that unlocks pending deltas:
```
Apply D1 (has 3 pending children)
→ Apply D2 (has 2 pending children)
  → Apply D4 (has 1 pending child)
    → Apply D7 (no children)
  → Apply D5 (no children)
→ Apply D3 (no children)

Total: 6 deltas applied in cascade
```

Worst case: O(N) where N = total pending that become ready

## Comparison with Similar Systems

### Git DAG

**Similarities**:
- Content-addressed deltas (commits)
- Parent references for causality
- Multiple heads = branches
- Merge commits with multiple parents

**Differences**:
- **Git**: Manual conflict resolution
- **Calimero**: Automatic CRDT merge
- **Git**: Requires full history
- **Calimero**: Works with partial history

### Vector Clocks / Lamport Timestamps

**Similarities**:
- Tracks causal relationships
- Detects concurrent events

**Differences**:
- **Vector clocks**: Per-node counters, abstract
- **Calimero DAG**: Explicit parent references, concrete
- **Vector clocks**: Determines `happens-before` relation
- **Calimero DAG**: Enables actual data merging

## Limitations and Future Work

### Current Limitations

1. **No delta request protocol** - If parent never arrives, delta stays pending forever
2. **No timeout implementation** - Pending buffer grows unbounded
3. **No persistence** - DAG state lost on restart (handled by wrapper)
4. **No garbage collection** - Old deltas retained forever

### Planned Improvements

1. **Request protocol integration**: When parent missing, request from peers
2. **Configurable timeouts**: Evict stale pending deltas
3. **Optional persistence**: Save/load DAG state
4. **Pruning mechanism**: Remove old deltas beyond checkpoint

## License

See [COPYRIGHT](../../COPYRIGHT) and [LICENSE.md](../../LICENSE.md) in the repository root.

