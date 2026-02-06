# DAG Integration Guide

How to integrate `calimero-dag` into your application.

---

## Overview

The DAG is designed to be **embedded** in your application. It provides the DAG logic, you provide:
1. **Storage applier**: How to apply deltas to your state
2. **Thread safety**: Wrapping in Arc/RwLock if needed
3. **Persistence**: Saving DAG state if needed

---

## Basic Integration

### Step 1: Add Dependency

```toml
[dependencies]
calimero-dag = { path = "../dag" }
tokio = { version = "1.0", features = ["full"] }
async-trait = "0.1"
```

### Step 2: Define Your Payload Type

```rust
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct MyPayload {
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum Action {
    Set { key: String, value: String },
    Delete { key: String },
}
```

### Step 3: Implement DeltaApplier

```rust
use calimero_dag::{CausalDelta, DeltaApplier, ApplyError};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct MyApplier {
    storage: Arc<RwLock<HashMap<String, String>>>,
}

#[async_trait]
impl DeltaApplier<MyPayload> for MyApplier {
    async fn apply(&self, delta: &CausalDelta<MyPayload>) -> Result<(), ApplyError> {
        let mut storage = self.storage.write().await;
        
        for action in &delta.payload.actions {
            match action {
                Action::Set { key, value } => {
                    storage.insert(key.clone(), value.clone());
                }
                Action::Delete { key } => {
                    storage.remove(key);
                }
            }
        }
        
        Ok(())
    }
}
```

### Step 4: Create and Use DAG

```rust
use calimero_dag::DagStore;

#[tokio::main]
async fn main() {
    // Create storage
    let storage = Arc::new(RwLock::new(HashMap::new()));
    
    // Create applier
    let applier = MyApplier {
        storage: storage.clone(),
    };
    
    // Create DAG
    let mut dag = DagStore::new([0; 32]);  // root = genesis hash
    
    // Add delta
    let delta = CausalDelta::new(
        [1; 32],
        dag.get_heads(),
        MyPayload {
            actions: vec![Action::Set {
                key: "hello".to_string(),
                value: "world".to_string(),
            }],
        },
        env::hlc_timestamp(),
    );
    
    let applied = dag.add_delta(delta, &applier).await.unwrap();
    println!("Applied: {}", applied);
    
    // Query storage
    let storage = storage.read().await;
    println!("Value: {:?}", storage.get("hello"));
}
```

---

## Node Layer Integration

### How Node Wraps DAG

The `calimero-node` crate wraps DAG with thread-safety and context management:

```rust
// crates/node/src/delta_store.rs
pub struct DeltaStore {
    // Thread-safe DAG
    dag: Arc<RwLock<DagStore<Vec<Action>>>>,
    
    // WASM applier
    applier: Arc<ContextStorageApplier>,
}

impl DeltaStore {
    pub fn new(
        root: [u8; 32],
        context_client: ContextClient,
        context_id: ContextId,
        our_identity: PublicKey,
    ) -> Self {
        let dag = Arc::new(RwLock::new(DagStore::new(root)));
        
        let applier = Arc::new(ContextStorageApplier {
            context_client,
            context_id,
            our_identity,
        });
        
        Self { dag, applier }
    }
    
    pub async fn add_delta(&self, delta: CausalDelta<Vec<Action>>) -> Result<bool> {
        // Acquire write lock
        let mut dag = self.dag.write().await;
        
        // Apply delta
        let applied = dag.add_delta(delta, &*self.applier).await?;
        
        // Update context DAG heads
        if applied {
            let heads = dag.get_heads();
            drop(dag);  // Release lock before external call
            
            self.applier.context_client
                .update_dag_heads(&self.applier.context_id, heads)?;
        }
        
        Ok(applied)
    }
    
    pub async fn get_heads(&self) -> Vec<[u8; 32]> {
        self.dag.read().await.get_heads()
    }
    
    pub async fn cleanup_stale(&self, max_age: Duration) -> usize {
        self.dag.write().await.cleanup_stale(max_age)
    }
    
    pub async fn pending_stats(&self) -> PendingStats {
        self.dag.read().await.pending_stats()
    }
}
```

**Key Patterns**:
1. **Arc<RwLock<>>**: Thread-safe shared ownership
2. **Arc<Applier>**: Share applier across threads
3. **Drop lock before external calls**: Avoid deadlocks
4. **Read lock for queries**: Allow concurrent reads

---

## ContextStorageApplier

### How WASM Execution Works

```rust
// crates/node/src/delta_store.rs
pub struct ContextStorageApplier {
    pub context_client: ContextClient,
    pub context_id: ContextId,
    pub our_identity: PublicKey,
}

#[async_trait]
impl DeltaApplier<Vec<Action>> for ContextStorageApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Serialize actions for WASM
        let artifact = borsh::to_vec(&StorageDelta::Actions(delta.payload.clone()))
            .map_err(|e| ApplyError::Application(e.to_string()))?;
        
        // Execute __calimero_sync_next in WASM
        let outcome = self.context_client
            .execute(
                &self.context_id,
                &self.our_identity,
                "__calimero_sync_next",
                artifact,
                vec![],
                None,
            )
            .await
            .map_err(|e| ApplyError::Application(e.to_string()))?;
        
        // Check for WASM errors
        outcome.returns
            .map_err(|e| ApplyError::Application(format!("WASM error: {:?}", e)))?;
        
        Ok(())
    }
}
```

**Flow**:
1. Serialize actions → WASM input
2. Execute `__calimero_sync_next` function
3. WASM applies actions to CRDT storage
4. Return success/failure

---

## Thread Safety Patterns

### Pattern 1: Single-Threaded (No Lock)

If your app is single-threaded (e.g., CLI tool):

```rust
pub struct MyApp {
    dag: DagStore<MyPayload>,
    applier: MyApplier,
}

impl MyApp {
    pub async fn add_delta(&mut self, delta: CausalDelta<MyPayload>) -> Result<bool> {
        // No lock needed - we own the DAG
        self.dag.add_delta(delta, &self.applier).await
    }
}
```

**Use when**: Single-threaded, no concurrent access

### Pattern 2: Multi-Threaded (Arc<RwLock<>>)

If you need concurrent access:

```rust
pub struct MyApp {
    dag: Arc<RwLock<DagStore<MyPayload>>>,
    applier: Arc<MyApplier>,
}

impl MyApp {
    pub async fn add_delta(&self, delta: CausalDelta<MyPayload>) -> Result<bool> {
        let mut dag = self.dag.write().await;
        dag.add_delta(delta, &*self.applier).await
    }
    
    pub async fn get_heads(&self) -> Vec<[u8; 32]> {
        self.dag.read().await.get_heads()
    }
}

impl Clone for MyApp {
    fn clone(&self) -> Self {
        Self {
            dag: self.dag.clone(),
            applier: self.applier.clone(),
        }
    }
}
```

**Use when**: Multi-threaded, need Clone, concurrent readers

### Pattern 3: Actor Model (Actix)

If using Actix actors:

```rust
use actix::prelude::*;

pub struct DagActor {
    dag: DagStore<MyPayload>,
    applier: MyApplier,
}

impl Actor for DagActor {
    type Context = Context<Self>;
}

#[derive(Message)]
#[rtype(result = "Result<bool>")]
pub struct AddDelta(pub CausalDelta<MyPayload>);

impl Handler<AddDelta> for DagActor {
    type Result = ResponseFuture<Result<bool>>;
    
    fn handle(&mut self, msg: AddDelta, _ctx: &mut Self::Context) -> Self::Result {
        let dag = &mut self.dag;
        let applier = &self.applier;
        let delta = msg.0;
        
        Box::pin(async move {
            dag.add_delta(delta, applier).await
        })
    }
}

// Usage:
let addr = DagActor::create(|_| DagActor { ... });
let result = addr.send(AddDelta(delta)).await??;
```

**Use when**: Actix-based architecture, message passing

---

## Persistence Patterns

### Pattern 1: No Persistence (In-Memory Only)

```rust
// DAG state lost on restart
pub struct MyApp {
    dag: DagStore<MyPayload>,
}

// On restart:
let app = MyApp {
    dag: DagStore::new(root),  // Fresh DAG
};
```

**Use when**: Short-lived processes, state in storage anyway

### Pattern 2: Persist Applied Set Only

```rust
pub struct PersistedDag {
    dag: DagStore<MyPayload>,
    db: Arc<Database>,
}

impl PersistedDag {
    pub async fn load(db: Arc<Database>) -> Result<Self> {
        let applied_ids: HashSet<[u8; 32]> = db.get_applied_deltas().await?;
        
        // Restore DAG state
        let mut dag = DagStore::new(root);
        for id in applied_ids {
            // Mark as applied without re-applying
            dag.mark_applied(id);  // Note: Not in current API
        }
        
        Ok(Self { dag, db })
    }
    
    pub async fn add_delta(&mut self, delta: CausalDelta<MyPayload>, applier: &impl DeltaApplier<MyPayload>) -> Result<bool> {
        let applied = self.dag.add_delta(delta.clone(), applier).await?;
        
        if applied {
            // Persist to database
            self.db.save_delta(&delta).await?;
        }
        
        Ok(applied)
    }
}
```

**Use when**: Need to recover applied state, storage handles deltas

### Pattern 3: Full DAG Persistence

```rust
// Save entire DAG to disk
pub async fn save_dag<T: Serialize>(
    dag: &DagStore<T>,
    path: &Path,
) -> Result<()> {
    let stats = dag.stats();
    let serialized = bincode::serialize(&stats)?;
    tokio::fs::write(path, serialized).await?;
    Ok(())
}

pub async fn load_dag<T: DeserializeOwned>(
    path: &Path,
) -> Result<DagStore<T>> {
    let data = tokio::fs::read(path).await?;
    let stats: DagStats = bincode::deserialize(&data)?;
    // Reconstruct DAG from stats
    // Note: Current API doesn't support this easily
    todo!("Implement DAG reconstruction")
}
```

**Use when**: Need full DAG recovery, including pending deltas

**Note**: Current DAG API doesn't expose methods for full persistence. This would require extending the API.

---

## Multi-Context Management

### Pattern: Context-Keyed DAGs

```rust
use dashmap::DashMap;

pub struct MultiContextDag {
    dags: Arc<DashMap<ContextId, Arc<RwLock<DagStore<Vec<Action>>>>>>,
    appliers: Arc<DashMap<ContextId, Arc<ContextStorageApplier>>>,
}

impl MultiContextDag {
    pub fn new() -> Self {
        Self {
            dags: Arc::new(DashMap::new()),
            appliers: Arc::new(DashMap::new()),
        }
    }
    
    pub async fn get_or_create(
        &self,
        context_id: &ContextId,
        context_client: ContextClient,
        our_identity: PublicKey,
    ) -> Arc<RwLock<DagStore<Vec<Action>>>> {
        if let Some(dag) = self.dags.get(context_id) {
            return dag.clone();
        }
        
        // Create new DAG
        let root = compute_context_root(context_id);
        let dag = Arc::new(RwLock::new(DagStore::new(root)));
        
        let applier = Arc::new(ContextStorageApplier {
            context_client,
            context_id: *context_id,
            our_identity,
        });
        
        self.dags.insert(*context_id, dag.clone());
        self.appliers.insert(*context_id, applier);
        
        dag
    }
    
    pub async fn add_delta(
        &self,
        context_id: &ContextId,
        delta: CausalDelta<Vec<Action>>,
    ) -> Result<bool> {
        let dag_ref = self.dags.get(context_id)
            .ok_or("Context not found")?;
        
        let applier_ref = self.appliers.get(context_id)
            .ok_or("Applier not found")?;
        
        let mut dag = dag_ref.write().await;
        dag.add_delta(delta, &**applier_ref).await
    }
}
```

**This is how node layer manages per-context DAGs**

---

## Custom Appliers

### Example 1: Logging Applier

```rust
pub struct LoggingApplier<T> {
    inner: Arc<dyn DeltaApplier<T>>,
}

#[async_trait]
impl<T> DeltaApplier<T> for LoggingApplier<T>
where
    T: Send + Sync + std::fmt::Debug,
{
    async fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError> {
        tracing::info!("Applying delta: {:?}", delta.id);
        
        let result = self.inner.apply(delta).await;
        
        match &result {
            Ok(_) => tracing::info!("Delta applied successfully"),
            Err(e) => tracing::error!("Delta apply failed: {}", e),
        }
        
        result
    }
}
```

### Example 2: Retry Applier

```rust
pub struct RetryApplier<T> {
    inner: Arc<dyn DeltaApplier<T>>,
    max_retries: u32,
}

#[async_trait]
impl<T> DeltaApplier<T> for RetryApplier<T>
where
    T: Send + Sync,
{
    async fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError> {
        for attempt in 1..=self.max_retries {
            match self.inner.apply(delta).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt < self.max_retries => {
                    tracing::warn!("Retry {} failed: {}", attempt, e);
                    tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }
}
```

### Example 3: Metrics Applier

```rust
pub struct MetricsApplier<T> {
    inner: Arc<dyn DeltaApplier<T>>,
}

#[async_trait]
impl<T> DeltaApplier<T> for MetricsApplier<T>
where
    T: Send + Sync,
{
    async fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError> {
        let start = Instant::now();
        
        let result = self.inner.apply(delta).await;
        
        let duration = start.elapsed();
        metrics::histogram!("dag.apply_duration", duration.as_secs_f64());
        
        match &result {
            Ok(_) => metrics::counter!("dag.apply_success", 1),
            Err(_) => metrics::counter!("dag.apply_failure", 1),
        }
        
        result
    }
}
```

---

## Monitoring Integration

### Export DAG Metrics

```rust
use prometheus::{register_gauge, register_counter, Gauge, Counter};

lazy_static! {
    static ref TOTAL_DELTAS: Gauge = register_gauge!(
        "dag_total_deltas",
        "Total deltas in DAG"
    ).unwrap();
    
    static ref PENDING_DELTAS: Gauge = register_gauge!(
        "dag_pending_deltas",
        "Pending deltas waiting for parents"
    ).unwrap();
    
    static ref HEAD_COUNT: Gauge = register_gauge!(
        "dag_head_count",
        "Number of DAG heads"
    ).unwrap();
}

// Update metrics periodically
pub async fn update_metrics(dag: &DagStore<MyPayload>) {
    let stats = dag.stats();
    let pending = dag.pending_stats();
    
    TOTAL_DELTAS.set(stats.total_deltas as f64);
    PENDING_DELTAS.set(pending.count as f64);
    HEAD_COUNT.set(stats.head_count as f64);
}
```

### Health Checks

```rust
pub async fn dag_health_check(dag: &DagStore<MyPayload>) -> HealthStatus {
    let stats = dag.stats();
    let pending = dag.pending_stats();
    
    if pending.count > 100 {
        return HealthStatus::NotAlive("Too many pending deltas".to_string());
    }
    
    if pending.oldest_age_secs > 300 {
        return HealthStatus::Degraded("Old pending deltas".to_string());
    }
    
    if stats.head_count > 10 {
        return HealthStatus::Warning("Many forks detected".to_string());
    }
    
    HealthStatus::Alive
}
```

---

## Testing Integration

### Integration Test Example

```rust
#[tokio::test]
async fn test_integration_with_storage() {
    // Setup
    let storage = Arc::new(RwLock::new(HashMap::new()));
    let applier = MyApplier { storage: storage.clone() };
    let mut dag = DagStore::new([0; 32]);
    
    // Test 1: Apply delta
    let delta1 = create_delta(1, vec![0], vec![
        Action::Set { key: "k1".into(), value: "v1".into() },
    ]);
    
    let applied = dag.add_delta(delta1, &applier).await.unwrap();
    assert!(applied);
    
    // Verify storage updated
    let storage = storage.read().await;
    assert_eq!(storage.get("k1"), Some(&"v1".to_string()));
    drop(storage);
    
    // Test 2: Dependent delta
    let delta2 = create_delta(2, vec![1], vec![
        Action::Set { key: "k2".into(), value: "v2".into() },
    ]);
    
    let applied = dag.add_delta(delta2, &applier).await.unwrap();
    assert!(applied);
    
    // Verify both keys present
    let storage = storage.read().await;
    assert_eq!(storage.len(), 2);
}
```

---

## Best Practices

### 1. Always Handle Apply Errors

```rust
// ❌ Bad: Ignore apply errors
dag.add_delta(delta, &applier).await.ok();

// ✅ Good: Handle errors
match dag.add_delta(delta, &applier).await {
    Ok(true) => { /* Applied */ },
    Ok(false) => { /* Pending */ },
    Err(DagError::ApplyFailed(e)) => {
        error!("Apply failed: {}", e);
        trigger_state_sync().await?;
    }
}
```

### 2. Request Missing Parents

```rust
// When delta is pending, request parents
let applied = dag.add_delta(delta, &applier).await?;
if !applied {
    for parent_id in dag.get_missing_parents() {
        tokio::spawn(request_from_network(parent_id));
    }
}
```

### 3. Monitor Pending Count

```rust
// Check pending count periodically
if dag.pending_stats().count > 100 {
    warn!("Too many pending deltas");
    trigger_state_sync().await?;
}
```

### 4. Clean Up Stale Deltas

```rust
// Run cleanup every 60 seconds
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;
        let evicted = dag.write().await.cleanup_stale(Duration::from_secs(300));
        if evicted > 0 {
            warn!("Evicted {} stale deltas", evicted);
        }
    }
});
```

### 5. Drop Locks Before External Calls

```rust
// ❌ Bad: Hold lock during external call
let mut dag = self.dag.write().await;
dag.add_delta(delta, &applier).await?;
self.network.broadcast(&delta).await?;  // Deadlock risk!

// ✅ Good: Drop lock first
let applied = {
    let mut dag = self.dag.write().await;
    dag.add_delta(delta.clone(), &applier).await?
};
if applied {
    self.network.broadcast(&delta).await?;
}
```

---

## Common Pitfalls

### Pitfall 1: Not Requesting Parents

**Problem**: Deltas stuck pending forever

**Solution**: Always request missing parents

### Pitfall 2: Holding Lock Too Long

**Problem**: Concurrent operations blocked

**Solution**: Minimize lock duration, drop before I/O

### Pitfall 3: No Cleanup

**Problem**: Unbounded memory growth

**Solution**: Run periodic cleanup

### Pitfall 4: Ignoring Apply Errors

**Problem**: Silent data loss

**Solution**: Handle errors, trigger recovery

---

## See Also

- [Architecture](architecture.md) - How DAG works internally
- [API Reference](api-reference.md) - Complete API docs
- [Testing Guide](testing-guide.md) - How to test integrations
- [Node README](../../node/README.md) - Example of full integration
