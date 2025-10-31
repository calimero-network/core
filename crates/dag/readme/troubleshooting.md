# DAG Troubleshooting Guide

Common issues and solutions when working with the DAG.

---

## Deltas Stuck in Pending

### Symptom

```rust
let stats = dag.pending_stats();
println!("{} deltas pending", stats.count);  // Keeps growing!
```

Pending count increases over time and never decreases.

### Causes

**1. Missing Parent Deltas**
- Parent delta lost in network transmission
- Network partition preventing delivery
- Peer doesn't have the parent either

**2. No Parent Request Protocol**
- DAG buffers pending deltas but doesn't request parents
- Waiting indefinitely for parents that will never arrive

**3. Circular Dependencies** (rare)
- Delta A waiting for B
- Delta B waiting for A
- Both stuck forever

### Solutions

#### Solution 1: Request Missing Parents

```rust
let applied = dag.add_delta(delta, &applier).await?;
if !applied {
    // Immediately request missing parents
    for parent_id in dag.get_missing_parents() {
        tokio::spawn(async move {
            if let Err(e) = request_from_peer(parent_id).await {
                warn!("Failed to request parent {}: {}", parent_id, e);
            }
        });
    }
}
```

#### Solution 2: Periodic Parent Requests

```rust
// Run every 10 seconds
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    loop {
        interval.tick().await;
        
        let missing = dag.read().await.get_missing_parents();
        if !missing.is_empty() {
            info!("Requesting {} missing parents", missing.len());
            for parent_id in missing {
                request_from_network(parent_id).await?;
            }
        }
    }
});
```

#### Solution 3: State Sync Fallback

If pending count exceeds threshold, trigger full state sync:

```rust
let stats = dag.pending_stats();
if stats.count > 100 || stats.oldest_age > Duration::from_secs(60) {
    warn!("Too many pending deltas - triggering state sync");
    trigger_state_sync().await?;
}
```

#### Solution 4: Cleanup Stale Deltas

```rust
// Run every 60 seconds
let evicted = dag.cleanup_stale(Duration::from_secs(300)); // 5 min
if evicted > 0 {
    warn!("Evicted {} stale pending deltas", evicted);
}
```

### Prevention

**Always implement parent request protocol**:
```rust
impl MyNode {
    async fn handle_delta(&self, delta: CausalDelta<T>) {
        let applied = self.dag.write().await.add_delta(delta, &self.applier).await?;
        
        if !applied {
            // CRITICAL: Request parents immediately!
            self.request_missing_parents().await;
        }
    }
    
    async fn request_missing_parents(&self) {
        let missing = self.dag.read().await.get_missing_parents();
        for parent_id in missing {
            self.send_request(parent_id).await?;
        }
    }
}
```

---

## Memory Growing Unbounded

### Symptom

Node memory usage increases over time, eventually causing OOM.

### Causes

**1. No Cleanup of Pending Deltas**
- Stale deltas never evicted
- Accumulate indefinitely

**2. Too Many Applied Deltas Kept**
- `applied` set grows without bound
- Each delta + payload consumes memory

**3. Large Payloads**
- Deltas contain large blobs or documents
- Memory per delta is high

### Solutions

#### Solution 1: Regular Cleanup

```rust
// Run cleanup every 60 seconds
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;
        
        let mut dag = dag.write().await;
        let evicted = dag.cleanup_stale(Duration::from_secs(300));
        
        if evicted > 0 {
            info!("Cleaned up {} stale deltas", evicted);
        }
    }
});
```

#### Solution 2: Set Limits

```rust
const MAX_PENDING: usize = 100;
const MAX_APPLIED: usize = 1000;

let stats = dag.pending_stats();
if stats.count > MAX_PENDING {
    error!("Too many pending deltas: {}", stats.count);
    // Trigger aggressive cleanup or state sync
    dag.cleanup_stale(Duration::from_secs(60)); // Shorter timeout
}
```

#### Solution 3: DAG Pruning (Advanced)

Implement pruning to remove old applied deltas:

```rust
// Keep only last N deltas
const KEEP_LAST_N: usize = 1000;

fn prune_old_deltas(dag: &mut DagStore<T>) {
    if dag.applied.len() > KEEP_LAST_N {
        // Get deltas in reverse topological order
        let all_deltas = get_all_deltas_topo_order(dag);
        
        // Keep last N, remove older ones
        for delta in all_deltas.iter().skip(KEEP_LAST_N) {
            dag.remove_delta(delta.id);
        }
    }
}
```

**Warning**: Only prune deltas you don't need for sync! Nodes joining later need history.

#### Solution 4: Offload to Persistent Storage

Instead of keeping all deltas in memory, offload to database:

```rust
struct DiskBackedDag {
    memory_dag: DagStore<T>,
    db: Arc<Database>,
}

impl DiskBackedDag {
    async fn add_delta(&mut self, delta: CausalDelta<T>) -> Result<bool> {
        // Apply in memory first
        let applied = self.memory_dag.add_delta(delta.clone(), &applier).await?;
        
        // Persist to disk
        if applied {
            self.db.save_delta(&delta).await?;
            
            // Prune from memory if too many
            if self.memory_dag.applied.len() > 1000 {
                self.prune_old_from_memory().await?;
            }
        }
        
        Ok(applied)
    }
}
```

### Monitoring

```rust
// Log memory stats periodically
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;
        
        let dag = dag.read().await;
        let stats = dag.pending_stats();
        
        info!(
            "DAG stats: applied={}, pending={}, heads={}, missing_parents={}",
            dag.applied.len(),
            stats.count,
            dag.get_heads().len(),
            stats.total_missing_parents
        );
    }
});
```

---

## Delta Apply Failures

### Symptom

```
Error: Failed to apply delta: Application(...)
```

Delta application fails, causing it to be lost.

### Causes

**1. Storage Errors**
- Database write failed
- Disk full
- Connection lost

**2. WASM Execution Errors**
- Invalid actions in payload
- State inconsistency
- Out of memory in WASM

**3. Serialization Errors**
- Corrupt payload data
- Version mismatch

### Solutions

#### Solution 1: Retry Logic

```rust
#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for RetryingApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        const MAX_RETRIES: u32 = 3;
        
        for attempt in 1..=MAX_RETRIES {
            match self.inner.apply(delta).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt < MAX_RETRIES => {
                    warn!("Apply attempt {} failed: {}", attempt, e);
                    tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                }
                Err(e) => return Err(e),
            }
        }
        
        unreachable!()
    }
}
```

#### Solution 2: Fallback to State Sync

If apply fails repeatedly, request full state sync:

```rust
match dag.add_delta(delta, &applier).await {
    Err(DagError::ApplyFailed(e)) => {
        error!("Apply failed permanently: {}", e);
        
        // Delta is lost - request state sync
        trigger_state_sync(context_id).await?;
    }
    _ => {}
}
```

#### Solution 3: Validate Before Apply

```rust
#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for ValidatingApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Validate payload first
        validate_actions(&delta.payload)
            .map_err(|e| ApplyError::Application(format!("Invalid actions: {}", e)))?;
        
        // Then apply
        self.inner.apply(delta).await
    }
}

fn validate_actions(actions: &[Action]) -> Result<()> {
    for action in actions {
        match action {
            Action::Add { data, .. } if data.is_empty() => {
                return Err("Empty data not allowed");
            }
            _ => {}
        }
    }
    Ok(())
}
```

### Prevention

**Log all apply failures for debugging**:
```rust
async fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError> {
    match self.try_apply(delta).await {
        Ok(()) => Ok(()),
        Err(e) => {
            error!(
                "Delta apply failed: delta_id={:?}, error={}",
                delta.id, e
            );
            
            // Log full delta for debugging
            if log_enabled!(Level::Debug) {
                debug!("Failed delta: {:?}", delta);
            }
            
            Err(e)
        }
    }
}
```

---

## Fork Not Resolving

### Symptom

```rust
let heads = dag.get_heads();
println!("{} heads", heads.len());  // Always > 1
```

Multiple heads persist even after merges.

### Causes

**1. No Merge Deltas Created**
- Nodes detect fork but don't create merge
- Automatic merge not implemented

**2. Merge Delta Not Propagating**
- Created but lost in network
- Not broadcast to all peers

**3. Different Nodes Creating Conflicting Merges**
- Node A creates merge [H1, H2]
- Node B creates merge [H1, H2] with different ID
- Creates more forks!

### Solutions

#### Solution 1: Automatic Merge Creation

```rust
// After receiving delta, check for forks
let heads = dag.get_heads();
if heads.len() > 1 {
    info!("Fork detected with {} heads", heads.len());
    
    // Create merge delta
    let merge_delta = create_merge_delta(heads);
    dag.add_delta(merge_delta, &applier).await?;
}
```

#### Solution 2: Coordinated Merge

Only designated node creates merge (e.g., lowest node_id):

```rust
let heads = dag.get_heads();
if heads.len() > 1 {
    // Deterministically choose merger based on heads
    let merger_id = heads.iter().min().unwrap();
    
    if merger_id == &my_node_id {
        // I'm responsible for merge
        create_and_broadcast_merge(heads).await?;
    } else {
        // Wait for designated node to merge
        info!("Waiting for node {:?} to merge", merger_id);
    }
}
```

#### Solution 3: Periodic Fork Resolution

```rust
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    loop {
        interval.tick().await;
        
        let heads = dag.read().await.get_heads();
        if heads.len() > 1 {
            warn!("Fork detected: {} heads", heads.len());
            resolve_fork(heads).await?;
        }
    }
});
```

---

## See Also

- [API Reference](api-reference.md) - Complete API docs
- [Architecture](architecture.md) - How DAG works  
- [Performance](performance.md) - Optimization tips
- [Integration](integration.md) - How to integrate with node

