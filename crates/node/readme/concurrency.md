# Concurrency & Availability

How Calimero nodes remain available during synchronization.

---

## Overview

Calimero nodes are designed for **continuous availability**. The node can:
- Respond to JSON-RPC queries while syncing
- Process requests for multiple contexts in parallel
- Handle network events without blocking API responses

**Key Design Principles**:
1. Sync runs as a background async task
2. Per-context locking (not global locks)
3. Thread-safe storage layer (RocksDB MVCC)
4. Channel-based event processing

---

## Concurrent Task Architecture

The node runs multiple independent tasks using `tokio::select!`:

```rust
// From: crates/node/src/run.rs
let mut sync = pin!(sync_manager.start());
let mut server = tokio::spawn(server);
let mut bridge = bridge_handle;

loop {
    tokio::select! {
        _ = &mut sync => {},          // Background sync
        res = &mut server => res??,   // HTTP/WebSocket server
        res = &mut bridge => { ... }  // Network event bridge
    }
}
```

```
┌─────────────────────────────────────────────────────────────────┐
│                         Main Event Loop                         │
│                        (tokio::select!)                         │
├─────────────────┬─────────────────┬─────────────────────────────┤
│  SyncManager    │  HTTP Server    │  NetworkEventBridge         │
│  (Background)   │  (User API)     │  (Network → NodeManager)    │
│                 │                 │                             │
│  • Interval     │  • REST API     │  • Forwards gossipsub       │
│    syncs        │  • WebSocket    │    messages to actors       │
│  • Delta DAG    │  • SSE events   │  • Channel-based (async)    │
│    catchup      │  • JSON-RPC     │                             │
└────────┬────────┴────────┬────────┴─────────────┬───────────────┘
         │                 │                      │
         ▼                 ▼                      ▼
┌─────────────────┐ ┌─────────────────┐ ┌─────────────────────────┐
│   NodeManager   │ │  ContextClient  │ │   DeltaStore (per ctx)  │
│   (Actix Actor) │ │  (Shared)       │ │   (RwLock-protected)    │
└─────────────────┘ └─────────────────┘ └─────────────────────────┘
```

---

## Per-Context Locking

### Design

Each context has its own mutex, allowing parallel operations across different contexts:

```rust
// From: crates/context/src/lib.rs
struct ContextMeta {
    meta: Context,
    lock: Arc<Mutex<ContextId>>,  // Per-context lock
}
```

### Implications

| Scenario | Behavior |
|----------|----------|
| Request A on Context 1, Request B on Context 2 | **Parallel** - different locks |
| Request A on Context 1, Request B on Context 1 | **Sequential** - same lock |
| Sync on Context 1, Query on Context 2 | **Parallel** - different locks |
| Sync on Context 1, Query on Context 1 | **Sequential** - waits for lock |

### Lock Acquisition

```rust
// Optimistic try_lock first, then async wait
fn lock(&self) -> Either<OwnedMutexGuard<ContextId>, impl Future<Output = OwnedMutexGuard<ContextId>>> {
    let Ok(guard) = self.lock.clone().try_lock_owned() else {
        return Either::Right(self.lock.clone().lock_owned());
    };
    Either::Left(guard)
}
```

---

## JSON-RPC Request Flow

### Execute Request

```
JSON-RPC POST /jsonrpc
    │
    ▼
┌─────────────────────────────────────┐
│ ServiceState { ctx_client }         │
│                                     │
│ ctx_client.execute(                 │
│   context_id,                       │
│   executor_public_key,              │
│   method,                           │
│   args                              │
│ )                                   │
└───────────────┬─────────────────────┘
                │
                ▼
┌─────────────────────────────────────┐
│ ContextManager (Actix Actor)        │
│                                     │
│ 1. get_or_fetch_context()           │
│ 2. context.lock()  ◄─── Per-context │
│ 3. WASM execute()                   │
│ 4. Release lock                     │
└───────────────┬─────────────────────┘
                │
                ▼
┌─────────────────────────────────────┐
│ RocksDB (Store)                     │
│ - Read state                        │
│ - Write changes (if mutating)       │
└─────────────────────────────────────┘
```

### What Gets Locked

| Operation | Acquires Lock | Why |
|-----------|---------------|-----|
| `execute()` (mutating method) | Yes | Modifies state, creates deltas |
| `execute()` (read-only query) | Yes | WASM runs, needs consistent view |
| `__calimero_sync_next` (sync apply) | Yes | Modifies state |
| Admin GET endpoints (list contexts) | No | Direct DB reads, no WASM |
| WebSocket subscriptions | No | Event streaming only |

---

## Sync Manager Concurrency

### Non-Blocking Design

```rust
// From: crates/node/src/sync/manager.rs
loop {
    tokio::select! {
        _ = next_sync.tick() => {
            // Periodic timer - non-blocking
        }
        Some(()) = async {
            loop { advance(&mut futs, &mut state).await? }
        } => {
            // Process completed syncs
        },
        Some((ctx, peer)) = ctx_sync_rx.recv() => {
            // On-demand sync request
        }
    }
    
    // Actual sync work happens AFTER select
    // Multiple syncs run concurrently via FuturesUnordered
}
```

### Concurrent Sync Operations

```rust
let mut futs = FuturesUnordered::new();

// Multiple contexts can sync in parallel
futs.push(timeout_at(deadline, self.perform_interval_sync(context_id, peer)));

// Only wait when at max concurrency
if futs.len() >= self.sync_config.max_concurrent {
    advance(&mut futs, &mut state).await;
}
```

---

## Storage Layer Concurrency

### RocksDB Thread Safety

The storage layer uses RocksDB which provides:
- **MVCC (Multi-Version Concurrency Control)**: Readers don't block writers
- **Snapshot isolation**: Consistent iteration during sync
- **Thread-safe handles**: Multiple threads can access simultaneously

```rust
// Snapshot iteration for consistent reads during sync
let mut iter = handle.iter_snapshot::<ContextStateKey>()?;

// Regular handle for point reads
let handle = self.context_client.datastore_handle();
let value = handle.get(&key)?;
```

### DeltaStore Locking

```rust
// From: crates/node/src/delta_store.rs
pub struct DeltaStore {
    dag: Arc<RwLock<CoreDagStore<Vec<Action>>>>,  // RwLock allows concurrent reads
    applier: Arc<ContextStorageApplier>,
    head_root_hashes: Arc<RwLock<HashMap<[u8; 32], [u8; 32]>>>,  // For merge detection
}
```

- **Multiple readers**: Queries can read DAG state concurrently
- **Single writer**: Delta application is serialized
- **Short critical sections**: Lock held only during DAG operations

---

## Availability During Different Sync States

### State Matrix

| Node State | JSON-RPC Availability | Notes |
|------------|----------------------|-------|
| **No sync in progress** | Full | Normal operation |
| **Periodic delta sync** | Full | Lock contention ~ms during apply |
| **Snapshot sync (uninitialized)** | `Uninitialized` error | Context unavailable until sync completes |
| **Snapshot sync (other contexts)** | Full | Only syncing context affected |
| **Delta catchup** | Full | Brief lock contention during apply |

### Uninitialized Context Protection

```rust
// From: crates/context/src/handlers/execute.rs
if !is_state_op && *context.meta.root_hash == [0; 32] {
    return ActorResponse::reply(Err(ExecuteError::Uninitialized));
}
```

Queries return `Uninitialized` error until first sync completes, preventing reads of empty state.

---

## Lock Contention Timeline

### Best Case (No Contention)

```
JSON-RPC Request ──[try_lock SUCCESS]──[WASM ~10ms]──[release]──
```

### With Contention (Same Context)

```
Request 1 ──────[LOCK]────────[WASM ~10ms]────────[release]─────
Request 2 ─────────[wait ~10ms]───────────────────[LOCK]──[WASM]──
Sync Apply ────────────────────────────────────────[wait]─[LOCK]──
```

### Different Contexts (Parallel)

```
Context A: ──────[LOCK]────[WASM ~10ms]────[release]──────
Context B: ──────[LOCK]────[WASM ~10ms]────[release]──────  (parallel!)
Context C: ──────[LOCK]────[WASM ~10ms]────[release]──────  (parallel!)
```

---

## Delta Buffering During Sync

### Problem

During snapshot sync, incoming deltas can't be applied (context uninitialized).

### Solution

```rust
// From: crates/node/src/lib.rs
pub(crate) struct SyncSession {
    pub state: SyncSessionState,
    pub delta_buffer: DeltaBuffer,            // Buffers incoming deltas
    pub last_drop_warning: Option<Instant>,   // Rate-limited warning tracking
}

// During snapshot sync (from handlers/state_delta.rs)
if node_state.should_buffer_delta(&context_id) {
    node_state.buffer_delta(&context_id, buffered);
    return Ok(());  // Non-blocking return
}
```

Deltas are buffered during snapshot sync and replayed after completion.

---

## Temporal Storage Layer (WASM Transactions)

### Problem

When WASM executes, it may:
- Read and write multiple keys
- Fail midway through execution
- Need to see its own uncommitted writes

Without proper handling, partial writes could corrupt state.

### Solution: Shadow Transactions

The `Temporal` layer provides transactional semantics by accumulating changes in memory before committing:

```rust
// From: crates/store/src/layer/temporal.rs
pub struct Temporal<'base, 'entry, L> {
    inner: &'base mut L,        // Underlying RocksDB store
    shadow: Transaction<'entry>, // In-memory pending changes
}
```

### How It Works

```
┌─────────────────────────────────────────────────────────────────┐
│                     WASM Execution                              │
│                                                                 │
│  storage.set("key1", value1)  ──┐                              │
│  storage.set("key2", value2)  ──┼──► Shadow Transaction        │
│  storage.get("key1")  ◄─────────┘    (in-memory BTreeMap)      │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
                                │
                    On success: │ storage.commit()
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                      RocksDB                                    │
│  All changes applied atomically via inner.apply(&shadow)        │
└─────────────────────────────────────────────────────────────────┘
```

### Read Shadowing

When reading, the temporal layer checks the shadow first:

```rust
// From: crates/store/src/layer/temporal.rs
fn get<K: AsKeyParts>(&self, key: &K) -> EyreResult<Option<Slice<'_>>> {
    match self.shadow.get(key) {
        Some(Operation::Delete) => Ok(None),     // Key deleted in this tx
        Some(Operation::Put { value }) => Ok(Some(value.into())), // Uncommitted write
        None => self.inner.get(key),             // Fall back to RocksDB
    }
}
```

This ensures WASM sees its own uncommitted writes immediately.

### Execution Flow

```rust
// From: crates/context/src/handlers/execute.rs
async fn execute_and_persist_state(...) {
    // 1. Create temporal storage (wraps RocksDB with shadow transaction)
    let storage = ContextStorage::from(datastore.clone(), context.id);
    let private_storage = ContextPrivateStorage::from(datastore, context.id);
    
    // 2. Execute WASM (all writes go to shadow transaction)
    let (outcome, storage, private_storage) = execute(
        guard, module, executor, method, input,
        storage, private_storage, ...
    ).await?;
    
    // 3. On error: return early, shadow transaction is dropped
    if outcome.returns.is_err() {
        return Ok((outcome, None));  // No commit - changes discarded
    }
    
    // 4. On success with state change: commit both storage layers
    if let Some(root_hash) = outcome.root_hash {
        let store = storage.commit()?;           // Atomic commit to RocksDB
        let _private = private_storage.commit()?;
    }
}
```

### Transaction Guarantees

| Property | Guarantee |
|----------|-----------|
| **Atomicity** | All writes commit together or none do |
| **Isolation** | Uncommitted changes invisible to other contexts |
| **Read-your-writes** | WASM sees its own pending changes |
| **Rollback on error** | Shadow dropped if WASM fails |

### Two Storage Types

```rust
// From: crates/context/src/handlers/execute/storage.rs
// Synchronized storage (replicated via deltas)
#[self_referencing]
pub struct ContextStorage {
    context_id: ContextId,
    store: Store,
    #[borrows(mut store)]
    inner: Temporal<'this, 'static, Store>,  // → ContextState column
    keys: RefCell<Vec<Arc<key::ContextState>>>,
}

// Private storage (node-local only)
#[self_referencing]
pub struct ContextPrivateStorage {
    context_id: ContextId,
    store: Store,
    #[borrows(mut store)]
    inner: Temporal<'this, 'static, Store>,  // → ContextPrivateState column
    keys: RefCell<Vec<Arc<key::ContextPrivateState>>>,
}
```

| Storage Type | Synced | Use Case |
|--------------|--------|----------|
| `ContextStorage` | Yes | Application state (CRDTs) |
| `ContextPrivateStorage` | No | Node-local caches, preferences |

### Why This Matters for Concurrency

1. **No partial writes visible**: Other contexts/queries never see incomplete state
2. **Safe rollback**: Failed WASM execution doesn't corrupt state
3. **Isolated mutations**: Each execution has its own shadow transaction
4. **Deterministic commits**: Same actions produce same state (CRDT property)

---

## Network Event Bridge

### Channel-Based Decoupling

```rust
// From: crates/node/src/network_event_processor.rs
pub struct NetworkEventBridge {
    receiver: NetworkEventReceiver,  // mpsc channel
    node_manager: Addr<NodeManager>,
    shutdown: Arc<Notify>,           // Graceful shutdown signal
}

loop {
    tokio::select! {
        event = self.receiver.recv() => {
            match event {
                Some(event) => self.node_manager.do_send(event),
                None => break,  // Channel closed
            }
        }
        _ = self.shutdown.notified() => break,
    }
}
```

- Channel capacity: 1000 events (configurable)
- Decouples network I/O from processing
- Provides backpressure when overwhelmed

---

## Performance Implications

### Read-Heavy Workloads

- **Impact**: Minimal
- **Why**: WASM queries are fast (~10ms), lock held briefly
- **Recommendation**: No special handling needed

### Write-Heavy + Sync

- **Impact**: Moderate contention
- **Why**: Delta applies compete for lock with user writes
- **Recommendation**: Batch writes when possible

### Multiple Contexts

- **Impact**: None (fully parallel)
- **Why**: Each context has independent lock
- **Recommendation**: Design applications with multiple contexts for parallelism

### Snapshot Sync (Bootstrap)

- **Impact**: Context unavailable for ~seconds
- **Why**: Full state transfer required
- **Recommendation**: Show loading state in UI during bootstrap

---

## Best Practices

### For Application Developers

1. **Handle `Uninitialized` errors gracefully** - Show loading state during initial sync
2. **Use multiple contexts** for independent data - Enables parallel access
3. **Prefer read-only queries** when possible - Lower contention
4. **Batch writes** - Fewer lock acquisitions

### For Node Operators

1. **Monitor sync duration** - Long syncs indicate network/peer issues
2. **Watch for lock contention** - High contention suggests write-heavy load
3. **Configure appropriate timeouts** - Balance responsiveness vs. reliability

---

## Configuration

### Sync Configuration

```rust
// From: crates/node/src/sync/config.rs
pub struct SyncConfig {
    pub timeout: Duration,            // Sync timeout (default: 30s)
    pub interval: Duration,           // Min between syncs (default: 5s)
    pub frequency: Duration,          // How often to check (default: 10s)
    pub max_concurrent: usize,        // Max parallel syncs (default: 30)
    pub snapshot_chunk_size: usize,   // Chunk size for full resync (default: 64KB)
    pub delta_sync_threshold: usize,  // Max delta gap before full resync
}
```

### Buffer Configuration

```rust
// Delta buffer for snapshot sync (from crates/node/primitives/src/delta_buffer.rs)
pub const DEFAULT_BUFFER_CAPACITY: usize = 10_000;  // 10,000 deltas per context

// Rate limit for overflow warnings (from crates/node/src/constants.rs)
pub const DELTA_BUFFER_DROP_WARNING_RATE_LIMIT_S: u64 = 5;
```

---

## See Also

- [Architecture](architecture.md) - Component structure
- [Sync Protocol](sync-protocol.md) - How sync works
- [Performance](performance.md) - Benchmarks and optimization
- [Troubleshooting](troubleshooting.md) - Common issues

---

## Navigation

- **Previous**: [Sync Configuration](sync-configuration.md)
- **Next**: [Troubleshooting](troubleshooting.md)
- **Up**: [Documentation Index](DOCUMENTATION_INDEX.md)
