# DAG API Reference

Complete API documentation for `calimero-dag`.

---

## Core Types

### `CausalDelta<T>`

A delta with parent references for causal ordering.

```rust
pub struct CausalDelta<T> {
    pub id: [u8; 32],           // Unique ID (content hash)
    pub parents: Vec<[u8; 32]>, // Parent IDs (causal dependencies)
    pub payload: T,             // Your delta content
    pub hlc: HybridTimestamp,   // Hybrid Logical Clock
}
```

**Construction**:
```rust
// Production
let delta = CausalDelta::new(
    compute_id(&parents, &payload),
    parents,
    payload,
    env::hlc_timestamp(),
);

// Testing
let delta = CausalDelta::new_test(id, parents, payload);
```

---

### `DagStore<T>`

Main DAG manager that tracks deltas and applies them in topological order.

```rust
pub struct DagStore<T> {
    // All seen deltas (both applied and pending)
    deltas: HashMap<[u8; 32], CausalDelta<T>>,
    
    // Successfully applied delta IDs
    applied: HashSet<[u8; 32]>,
    
    // Deltas waiting for parents
    pending: HashMap<[u8; 32], PendingDelta<T>>,
    
    // Current DAG tips
    heads: HashSet<[u8; 32]>,
}
```

---

## Adding Deltas

### `add_delta`

Add a delta to the DAG. Returns `true` if applied immediately, `false` if buffered as pending.

```rust
pub async fn add_delta(
    &mut self,
    delta: CausalDelta<T>,
    applier: &impl DeltaApplier<T>,
) -> Result<bool, DagError>
```

**Behavior**:
1. Check if delta already exists → return `DuplicateDelta` error
2. Check if all parents are in `applied` set
3. **If parents ready**: Apply immediately, update heads, check for cascade
4. **If parents missing**: Buffer as pending, return `false`

**Example**:
```rust
let mut dag = DagStore::new([0; 32]);
let applier = MyApplier::new();

let delta = CausalDelta::new(
    [1; 32],
    dag.get_heads(),  // Build on current tips
    my_payload,
    env::hlc_timestamp(),
);

match dag.add_delta(delta, &applier).await? {
    true => println!("Applied immediately"),
    false => {
        println!("Pending - missing parents");
        let missing = dag.get_missing_parents();
        // Request missing parents from network
    }
}
```

**Error Cases**:
- `DagError::DuplicateDelta`: Delta ID already exists in DAG
- `DagError::ApplyFailed`: Applier returned error

**Cascade Behavior**:
When a delta is applied, the DAG automatically checks if any pending deltas are now ready:

```
add_delta(D1) [applied]
  → Check pending for deltas with parent D1
  → Found D2 waiting for D1
  → Recursively apply D2
    → Check pending for deltas with parent D2
    → Found D3 waiting for D2  
    → Recursively apply D3
    → ...
```

This means one `add_delta` call can trigger multiple applications!

---

## Queries

### `get_heads`

Get current DAG tips (deltas with no children).

```rust
pub fn get_heads(&self) -> Vec<[u8; 32]>
```

**Returns**: List of head IDs (typically 1, sometimes 2-10 during forks)

**Example**:
```rust
let heads = dag.get_heads();
println!("Current heads: {} (fork detected!)", heads.len());

// Use heads as parents for next delta
let next_delta = CausalDelta::new(
    new_id,
    heads,  // Multiple parents = merge delta
    payload,
    hlc,
);
```

---

### `get_delta`

Get a specific delta by ID.

```rust
pub fn get_delta(&self, id: &[u8; 32]) -> Option<&CausalDelta<T>>
```

**Returns**: Reference to delta if exists (either applied or pending)

**Example**:
```rust
if let Some(delta) = dag.get_delta(&id) {
    println!("Found delta with {} parents", delta.parents.len());
} else {
    println!("Delta not in DAG");
}
```

---

### `has_delta`

Check if delta exists in DAG (applied or pending).

```rust
pub fn has_delta(&self, id: &[u8; 32]) -> bool
```

**Example**:
```rust
if dag.has_delta(&suspected_duplicate) {
    // Skip processing, already have it
}
```

---

### `is_applied`

Check if delta has been successfully applied.

```rust
pub fn is_applied(&self, id: &[u8; 32]) -> bool
```

**Example**:
```rust
for parent in &delta.parents {
    if !dag.is_applied(parent) {
        println!("Missing parent: {:?}", parent);
    }
}
```

---

### `get_missing_parents`

Get all missing parent IDs that are preventing pending deltas from being applied.

```rust
pub fn get_missing_parents(&self) -> Vec<[u8; 32]>
```

**Returns**: List of parent IDs that are referenced but not in the DAG

**Example**:
```rust
let missing = dag.get_missing_parents();
if !missing.is_empty() {
    println!("Need to request {} deltas from network", missing.len());
    for parent_id in missing {
        request_from_peer(parent_id).await?;
    }
}
```

---

### `pending_stats`

Get statistics about pending deltas.

```rust
pub fn pending_stats(&self) -> PendingStats

pub struct PendingStats {
    pub count: usize,                 // Number of pending deltas
    pub total_missing_parents: usize, // Total missing parent refs
    pub oldest_age: Option<Duration>, // Age of oldest pending delta
}
```

**Example**:
```rust
let stats = dag.pending_stats();
if stats.count > 100 {
    warn!("Too many pending deltas: {}", stats.count);
}

if let Some(age) = stats.oldest_age {
    if age > Duration::from_secs(60) {
        warn!("Oldest pending delta: {:?}", age);
        // Maybe trigger cleanup or state sync
    }
}
```

---

###get_deltas_since`

Get all deltas since a given delta ID (topological order).

```rust
pub fn get_deltas_since(&self, since: &[u8; 32]) -> Vec<CausalDelta<T>>
```

**Returns**: All deltas that are descendants of `since`, in topological order

**Example**:
```rust
// Sync protocol: peer asks for deltas since their last known
let their_head = [42; 32];
let missing_deltas = dag.get_deltas_since(&their_head);

// Send to peer
send_to_peer(&missing_deltas).await?;
```

---

## Cleanup

### `cleanup_stale`

Remove pending deltas older than the given timeout.

```rust
pub fn cleanup_stale(&mut self, timeout: Duration) -> usize
```

**Returns**: Number of evicted deltas

**Example**:
```rust
// Run every 60 seconds
let evicted = dag.cleanup_stale(Duration::from_secs(300)); // 5 min
if evicted > 0 {
    warn!("Evicted {} stale pending deltas", evicted);
}
```

**Why cleanup is needed**:
- Pending deltas consume memory
- Parents might never arrive (network partition, lost packet)
- Prevents unbounded memory growth

**Recommendation**: Run cleanup every 60 seconds with 300-second timeout.

---

## DeltaApplier Trait

Implement this trait to define how deltas are applied.

```rust
#[async_trait::async_trait]
pub trait DeltaApplier<T> {
    async fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError>;
}
```

### Example: Simple In-Memory Applier

```rust
use std::sync::{Arc, Mutex};

struct InMemoryApplier {
    applied: Arc<Mutex<Vec<[u8; 32]>>>,
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<u8>> for InMemoryApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<u8>>) -> Result<(), ApplyError> {
        // Your logic here
        println!("Applying delta: {:?}", delta.id);
        
        self.applied.lock().unwrap().push(delta.id);
        Ok(())
    }
}
```

### Example: Storage-Backed Applier

```rust
struct StorageApplier {
    db: Arc<Database>,
    context_id: ContextId,
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for StorageApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Serialize actions
        let artifact = borsh::to_vec(&delta.payload)
            .map_err(|e| ApplyError::Application(e.to_string()))?;
        
        // Execute in WASM (sync operation)
        let outcome = execute_wasm_sync(
            &self.context_id,
            "__calimero_sync_next",
            &artifact,
        ).map_err(|e| ApplyError::Application(e.to_string()))?;
        
        // Persist to database
        self.db.save_delta(delta.id, &delta.payload)
            .await
            .map_err(|e| ApplyError::Application(e.to_string()))?;
        
        Ok(())
    }
}
```

---

## Error Handling

### `DagError`

```rust
pub enum DagError {
    DuplicateDelta([u8; 32]),      // Delta already exists
    ApplyFailed(ApplyError),        // Applier returned error
}
```

### `ApplyError`

```rust
pub enum ApplyError {
    Application(String),  // Application-specific error
}
```

**Handling**:
```rust
match dag.add_delta(delta, &applier).await {
    Ok(true) => println!("Applied"),
    Ok(false) => println!("Pending"),
    Err(DagError::DuplicateDelta(id)) => {
        // Ignore - already have it
    }
    Err(DagError::ApplyFailed(e)) => {
        error!("Failed to apply: {}", e);
        // Delta is lost! May need to request state sync
    }
}
```

---

## Best Practices

### 1. Always Request Missing Parents

```rust
let applied = dag.add_delta(delta, &applier).await?;
if !applied {
    // Request missing parents immediately
    for parent_id in dag.get_missing_parents() {
        spawn(request_from_network(parent_id));
    }
}
```

### 2. Run Periodic Cleanup

```rust
// Every 60 seconds
let evicted = dag.cleanup_stale(Duration::from_secs(300));
if evicted > 0 {
    warn!("Evicted {} stale deltas", evicted);
}
```

### 3. Monitor Pending Count

```rust
let stats = dag.pending_stats();
if stats.count > 100 {
    warn!("Too many pending deltas - possible network issue");
    // Trigger state sync fallback
}
```

### 4. Handle Apply Failures Gracefully

```rust
// In your DeltaApplier implementation
async fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError> {
    match try_apply(delta).await {
        Ok(()) => Ok(()),
        Err(e) => {
            error!("Apply failed: {}", e);
            // Log for debugging
            log_delta_failure(delta, &e);
            Err(ApplyError::Application(e.to_string()))
        }
    }
}
```

---

## See Also

- [Architecture](architecture.md) - How DAG works internally
- [Testing Guide](testing-guide.md) - How to test DAG behavior  
- [Troubleshooting](troubleshooting.md) - Common issues
- [Performance](performance.md) - Complexity analysis

