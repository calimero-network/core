# Node Layer Performance Analysis & Optimization Opportunities

## Current Architecture

### Data Flow for Incoming Delta
```
Gossipsub Message
  ↓
NetworkEvent::Message (line 63, network_event.rs)
  ↓
borsh::from_slice<BroadcastMessage> (line 69)
  ↓
handle_state_delta() spawned (line 104)
  ↓
Decrypt artifact (lines 73-76, state_delta.rs)
  ↓
borsh::from_slice<StorageDelta> (line 79-80)
  ↓
dag.write().await (line 115, delta_store.rs) ⚠️ LOCK CONTENTION
  ↓
WASM execution (__calimero_sync_next) (line 43-54, delta_store.rs)
  ↓
serde_json::from_slice<Vec<ExecutionEvent>> #1 (line 214, state_delta.rs) ⚠️ DUPLICATE
  ↓
Sequential handler execution (line 216 for loop) ⚠️ NOT PARALLEL
  ↓
serde_json::from_slice<Vec<ExecutionEvent>> #2 (line 276) ⚠️ DUPLICATE
  ↓
WebSocket emit
```

## Identified Bottlenecks

### 🔴 CRITICAL: Double Event Deserialization

**Location**: `crates/node/src/handlers/state_delta.rs:214, 276`

**Problem**:
```rust
// Line 214: Deserialize for handlers
match serde_json::from_slice::<Vec<ExecutionEvent>>(events_data) {
    Ok(events_payload) => {
        // Execute handlers
    }
}

// Line 276: Deserialize AGAIN for WebSocket
match serde_json::from_slice::<Vec<ExecutionEvent>>(&events_data) {
    Ok(events_payload) => {
        // Emit to WebSocket
    }
}
```

**Impact**: 
- Wastes CPU on parsing the same JSON twice
- ~10-50ms overhead per delta with events (debug), ~1-5ms (release)

**Fix**: Deserialize once, share the result:
```rust
// Deserialize once if we have events
let events_payload = if let Some(events_data) = &events {
    match serde_json::from_slice::<Vec<ExecutionEvent>>(events_data) {
        Ok(payload) => Some(payload),
        Err(e) => {
            warn!(%context_id, %e, "Failed to deserialize events");
            None
        }
    }
} else {
    None
};

// Use for handlers
if applied {
    if let Some(ref payload) = events_payload {
        if author_id != our_identity {
            execute_event_handlers_from_payload(
                &node_clients.context,
                &context_id,
                &our_identity,
                payload,
            ).await?;
        }
    }
}

// Use for WebSocket (no re-deserialization!)
if let Some(payload) = events_payload {
    emit_state_mutation_event(&node_clients.node, &context_id, root_hash, payload)?;
}
```

### 🟡 MEDIUM: Sequential Handler Execution

**Location**: `crates/node/src/handlers/state_delta.rs:216-251`

**Problem**:
```rust
for event in &events_payload {
    if let Some(handler_name) = &event.handler {
        match context_client.execute(...).await { ... }  // SEQUENTIAL!
    }
}
```

**Impact**:
- If 5 handlers each take 100ms → total 500ms
- Could be ~100ms if parallel

**Fix**: Use `futures::future::join_all()` or `FuturesUnordered`:
```rust
use futures_util::stream::{FuturesUnordered, StreamExt};

let mut handler_futs = FuturesUnordered::new();

for event in &events_payload {
    if let Some(handler_name) = &event.handler {
        let fut = context_client.execute(
            context_id,
            our_identity,
            handler_name.clone(),
            event.data.clone(),
            vec![],
            None,
        );
        handler_futs.push(fut);
    }
}

// Execute all handlers concurrently
while let Some(result) = handler_futs.next().await {
    match result {
        Ok(_) => debug!("Handler executed successfully"),
        Err(err) => warn!(?err, "Handler execution failed"),
    }
}
```

**Trade-off**: 
- Handlers may execute in different order (might affect causality)
- Consider if handlers have dependencies

### 🟡 MEDIUM: RwLock Contention in DAG

**Location**: `crates/node/src/delta_store.rs:115`

**Problem**:
```rust
pub async fn add_delta(&self, delta: CausalDelta<Vec<Action>>) -> Result<bool> {
    let mut dag = self.dag.write().await;  // EXCLUSIVE LOCK!
    let result = dag.add_delta(delta, &*self.applier).await?;
    // ... holds lock through WASM execution!
}
```

**Impact**:
- Under high message rate (50 deltas/sec), write lock blocks all reads
- Queries for `get_heads()`, `get_missing_parents()` wait for WASM execution
- Could delay subsequent delta processing by 10-100ms

**Current mitigation**: Line 121 drops lock early (good!)

**Potential fix**: Use message passing or batching:
```rust
// Batch deltas and apply in batches
// Or use a lock-free DAG structure (harder)
```

**Assessment**: Current code is reasonable, lock is dropped before external calls. 
**Don't change unless profiling shows this is a bottleneck.**

### 🟢 LOW: Sequential Missing Delta Requests

**Location**: `crates/node/src/handlers/state_delta.rs:305-362`

**Current**: Deltas requested one-by-one in sequence

**Potential fix**: Parallel requests using `join_all` or `FuturesUnordered`

**Impact**: ~50-200ms saved when catching up on 5-10 missing deltas

**Trade-off**: More network connections, could overwhelm peer

### 🟢 LOW: Channel Buffer Sizes

**Location**: `crates/node/src/run.rs:87, 89`

```rust
let (event_sender, _) = broadcast::channel(32);  // WebSocket events
let (ctx_sync_tx, ctx_sync_rx) = mpsc::channel(16);  // Sync requests
```

**Concern**: Under burst traffic, could cause backpressure

**Recommendation**:
```rust
let (event_sender, _) = broadcast::channel(256);  // Handle more concurrent WebSocket clients
let (ctx_sync_tx, ctx_sync_rx) = mpsc::channel(64);  // Handle burst sync requests
```

**Impact**: Prevents dropped events/sync requests during bursts

### 🟢 LOW: Cleanup Intervals

**Location**: `crates/node/src/lib.rs:171, 176, 227`

**Current**:
- Blob cache eviction: every 300s (5 min)
- Stale delta cleanup: every 60s (1 min)
- Hash heartbeat: every 30s

**These are reasonable for production.** No changes recommended.

## Recommended Optimizations (Priority Order)

### Priority 1: Fix Double Deserialization ⚡
- **Impact**: HIGH (wastes ~5-50ms per delta with events)
- **Effort**: LOW (simple refactor)
- **Risk**: NONE
- **Files**: `state_delta.rs`

### Priority 2: Increase Channel Buffers
- **Impact**: MEDIUM (prevents backpressure during bursts)
- **Effort**: TRIVIAL (2 line change)
- **Risk**: NONE (just uses more memory)
- **Files**: `run.rs`

### Priority 3: Parallel Handler Execution (OPTIONAL)
- **Impact**: MEDIUM (~100-400ms saved for multiple handlers)
- **Effort**: MEDIUM (need to ensure handler independence)
- **Risk**: MEDIUM (could break causality if handlers depend on each other)
- **Files**: `state_delta.rs`
- **⚠️ ONLY do this if handlers are independent**

### Priority 4: Parallel Missing Delta Requests (OPTIONAL)
- **Impact**: LOW-MEDIUM (only helps during catch-up)
- **Effort**: MEDIUM
- **Risk**: LOW (could overwhelm peers)
- **Files**: `state_delta.rs`

## DashMap vs RwLock Analysis

**Good news**: You're already using `DashMap` for high-contention data structures:

```rust
pub(crate) struct NodeState {
    pub(crate) blob_cache: Arc<DashMap<BlobId, CachedBlob>>,      // ✅ Lock-free
    pub(crate) delta_stores: Arc<DashMap<ContextId, DeltaStore>>, // ✅ Lock-free
}
```

**DashMap advantages**:
- Lock-free reads (no contention)
- Fine-grained locking (per-key, not whole map)
- Concurrent reads and writes

**RwLock in DeltaStore.dag**: 
- Necessary because CoreDagStore needs exclusive access during delta application
- Already optimized (lock dropped early, line 121)

## Memory Usage Analysis

**Current**:
```rust
// Per context
DeltaStore: ~8KB base + pending deltas
  - dag: ~1KB per pending delta (avg)
  - Cleanup threshold: 100 pending deltas max (line 209, lib.rs)
  - Max memory: ~100KB per context

// Per blob in cache
CachedBlob: blob size + 24 bytes overhead
  - Eviction: 300s TTL (line 94, lib.rs)
  - Max cache: unbounded (could be issue for many large blobs)

// Total per node with 10 contexts, 100 blobs
~= 10 * 100KB + 100 * avg_blob_size
~= 1MB + blob data
```

**Recommendation**: Add max blob cache size (current ly time-based only):
```rust
const MAX_BLOB_CACHE_SIZE: usize = 100;  // Max 100 blobs in cache
const MAX_BLOB_CACHE_BYTES: usize = 500 * 1024 * 1024;  // 500MB total
```

## Concurrency Analysis

**Current concurrency model**:
1. ✅ Network events spawned independently (line 104, network_event.rs)
2. ✅ DashMap allows concurrent delta store access per context
3. ❌ Event handlers execute sequentially within each delta
4. ✅ Blob operations spawned independently (line 238, network_event.rs)
5. ✅ Cleanup operations spawned (line 180, lib.rs)

**Bottleneck**: Only handler execution is sequential.

## Recommendations Summary

| Optimization | Impact | Effort | Risk | Recommend? |
|--------------|--------|--------|------|------------|
| **Fix double deserialization** | HIGH | LOW | NONE | ✅ **YES** |
| **Increase channel buffers** | MEDIUM | TRIVIAL | NONE | ✅ **YES** |
| **Add blob cache size limit** | LOW | LOW | NONE | ✅ **YES** |
| **Parallel handler execution** | MEDIUM | MEDIUM | MEDIUM | ⚠️ **MAYBE** (test first) |
| **Parallel missing delta requests** | LOW | MEDIUM | LOW | ⏸️ **LATER** (not urgent) |

## Next Steps

1. Implement double deserialization fix
2. Increase channel buffers
3. Add blob cache size limit
4. Profile in production to validate assumptions
5. Consider parallel handlers if profiling shows benefit

