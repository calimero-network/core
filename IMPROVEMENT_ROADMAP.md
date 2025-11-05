# Calimero Core - Improvement Roadmap

## Summary
This document outlines technical debt, architectural issues, and improvements identified during debugging and refactoring sessions (Nov 2024).

---

## 🔥 Critical Issues (Fixed)

### ✅ 1. Authentication Protocol Mismatch
**Status**: Fixed in commit `87ef3881`

**Problem**: Missing `prove_identity()` calls in delta request initiators caused protocol mismatches.

**Fix**: Added `prove_identity()` after all `DeltaRequest` sends in `manager.rs`.

---

### ✅ 2. Unbounded Context Cache (Memory Leak)
**Status**: Fixed in commit `86359ce9`

**Problem**: `BTreeMap<ContextId, ContextMeta>` grew without bounds, causing OOM in production.

**Fix**: Replaced with `LruCache` (max 1000 contexts) with automatic eviction.

---

### ✅ 3. Sync Error Propagation
**Status**: Fixed in commit `66ec3240`

**Problem**: `sync()` failures were silently swallowed, making debugging impossible.

**Fix**: 
- Added `sync_and_wait()` with `oneshot` channels for result tracking
- Emit `NodeEvent::Sync` events (Started/Completed/Failed)
- Propagate errors in `join_context` instead of fire-and-forget

---

### ✅ 4. SyncManager Concurrency Issues
**Status**: Fixed in commit `239666ab` (earlier)

**Problem**: `tokio::select!` processed one message at a time, causing queue starvation.

**Fix**: Drain all pending messages with `try_recv()` and batch sync requests.

---

## 🚨 High Priority Issues

### 1. **Unsafe Code in `create_context.rs` (REMOVED in LRU fix)**
**Location**: Previously `create_context.rs:190-197`

**Problem**: 
```rust
// REMOVED - this was unsafe transmute!
let entry = unsafe {
    mem::transmute::<_, btree_map::VacantEntry<'static, ContextId, ContextMeta>>(entry)
};
```

**Status**: ✅ Removed during LRU cache refactoring - no longer needed!

---

### 2. **Connection Pooling Missing**
**Location**: `crates/node/src/sync/*.rs`

**Problem**: Every sync protocol creates a new TCP connection.

**Impact**:
- High latency for frequent syncs
- Connection overhead (handshake, TLS if applicable)
- Resource waste

**Proposal**:
```rust
struct ConnectionPool {
    connections: HashMap<PeerId, Arc<Mutex<Stream>>>,
    max_idle: Duration,
}

impl ConnectionPool {
    async fn get_or_create(&mut self, peer: &PeerId) -> Stream {
        // Reuse existing or create new
    }
}
```

**Effort**: Medium (2-3 days)

---

### 3. **No Metrics/Observability**
**Location**: Throughout codebase

**Missing Metrics**:
- ✅ Sync success/failure rates (added via events)
- ❌ Cache hit/miss rates (LRU)
- ❌ Connection pool stats
- ❌ Delta store size
- ❌ Context count per node
- ❌ DAG head count distribution

**Proposal**:
```rust
struct ContextMetrics {
    cache_hits: Counter,
    cache_misses: Counter,
    cache_evictions: Counter,
    active_contexts: Gauge,
}
```

**Effort**: Low (1 day)

---

### 4. **Actor Pattern Overuse**
**Location**: `crates/context/src/lib.rs`, all handlers

**Problem**: 
- Everything is an actor with message passing
- Makes debugging harder (async boundaries everywhere)
- Serialization overhead
- Complex error handling

**Example**:
```rust
// Current: Message passing with ActorResponse
impl Handler<CreateContextRequest> for ContextManager {
    fn handle(&mut self, ...) -> ActorResponse<...> {
        ActorResponse::r#async(task.into_actor(self).map_ok(...))
    }
}

// Simpler: Direct async fn
impl ContextManager {
    async fn create_context(&mut self, ...) -> Result<CreateContextResponse> {
        // Direct implementation
    }
}
```

**Why Actors?**: Originally for concurrency, but now we have one actor per context anyway (via locks).

**Proposal**: Evaluate if actors are still needed, or if we can use simpler async fns with Arc<Mutex<>> for shared state.

**Effort**: High (1-2 weeks) - major refactoring

---

### 5. **No Graceful Shutdown**
**Location**: `crates/node/src/run.rs`, `crates/context/src/lib.rs`

**Problem**: No cleanup on shutdown - contexts, connections, tasks all just drop.

**Missing**:
- Flush pending deltas to disk
- Close connections gracefully
- Cancel ongoing syncs
- Save cache state

**Proposal**:
```rust
impl ContextManager {
    async fn shutdown(&mut self) -> Result<()> {
        // Flush all contexts to DB
        for (id, meta) in self.contexts.iter() {
            self.context_client.save_context(&meta.meta)?;
        }
        Ok(())
    }
}
```

**Effort**: Medium (3-5 days)

---

## 📊 Medium Priority Issues

### 6. **BTreeMap for Applications** (Same issue as contexts!)
**Location**: `crates/context/src/lib.rs:67`

```rust
applications: BTreeMap<ApplicationId, Application>,  // UNBOUNDED!
```

**Problem**: Same memory leak as contexts - grows without bounds.

**Fix**: Apply same LRU pattern.

**Effort**: Low (few hours - copy context cache pattern)

---

### 7. **No Request Timeouts**
**Location**: Throughout RPC handlers

**Problem**: Handlers can hang forever waiting for external services.

**Example**:
```rust
// No timeout!
let config_client = external_client.config();
let proxy_contract = config_client.get_proxy_contract().await?;
```

**Fix**:
```rust
tokio::time::timeout(Duration::from_secs(30), 
    config_client.get_proxy_contract()
).await??;
```

**Effort**: Low (1 day - add timeouts to all external calls)

---

### 8. **Inconsistent Error Types**
**Location**: Everywhere

**Problem**: Mix of `eyre::Result`, `std::io::Result`, `actix::MailboxError`, custom errors.

**Example**:
```rust
// What error type is this??
pub async fn sync(...) -> eyre::Result<SyncProtocol> { ... }
```

**Proposal**: Define domain-specific error types:
```rust
#[derive(thiserror::Error, Debug)]
pub enum ContextError {
    #[error("Context not found: {0}")]
    NotFound(ContextId),
    
    #[error("Sync failed: {0}")]
    SyncFailed(String),
    
    #[error("Database error: {0}")]
    Database(#[from] rocksdb::Error),
}
```

**Effort**: Medium (1 week - needs careful migration)

---

### 9. **No Rate Limiting**
**Location**: Sync protocols, RPC handlers

**Problem**: No protection against spam or DoS.

**Example**:
- Node can spam sync requests
- Malicious peer can request all deltas
- No backoff for failed syncs (we added exponential backoff, but no global limit)

**Proposal**:
```rust
struct RateLimiter {
    max_requests_per_second: usize,
    per_peer: HashMap<PeerId, TokenBucket>,
}
```

**Effort**: Medium (3-5 days)

---

### 10. **Hardcoded Configuration**
**Location**: `crates/node/src/run.rs:153`, `crates/context/src/lib.rs:111`

```rust
// Hardcoded!
contexts: LruCache::new(NonZeroUsize::new(1000).expect("1000 > 0")),
ctx_sync_tx: mpsc::channel(256),  // Why 256?
```

**Proposal**: Make configurable:
```toml
[cache]
max_contexts = 1000
max_applications = 500

[sync]
queue_size = 256
timeout_secs = 30
```

**Effort**: Low (1-2 days)

---

## 🔧 Low Priority / Nice-to-Have

### 11. **Clone Everywhere**
**Location**: All handlers

**Problem**: Excessive cloning of large structs.

**Example**:
```rust
let context_meta_for_map_ok = context_meta.clone();
let context_meta_for_map_err = context_meta.clone();
```

**Why**: Closures in futures need owned data.

**Fix**: Use `Arc<>` for large structs, or restructure to avoid clones.

**Effort**: Medium (ongoing - opportunistic improvements)

---

### 12. **No Integration Tests**
**Location**: Missing

**Problem**: Only E2E tests exist. No mid-level integration tests for:
- Context lifecycle (create -> join -> execute -> delete)
- Sync protocols (delta catchup, full resync)
- Cache behavior (eviction, reloading)

**Effort**: Medium (1 week to build test harness)

---

### 13. **Inconsistent Logging**
**Location**: Everywhere

**Problem**:
- Mix of `info!`, `debug!`, `warn!`, `error!`
- No structured logging for important events
- Too verbose in some places, too quiet in others

**Example**:
```rust
// Too verbose for normal operation
debug!("Loaded context from database context_id={}", context_id);

// Not enough context
error!("Sync failed");  // What context? What peer? What error?
```

**Proposal**: Use structured logging with spans:
```rust
#[instrument(skip(self), fields(context_id = %context_id))]
async fn sync_context(&self, context_id: &ContextId) -> Result<()> {
    info!("Starting sync");
    // ...
    error!(error = %e, "Sync failed");
}
```

**Effort**: Medium (1 week - review and update all log statements)

---

### 14. **No Benchmarks**
**Location**: Missing

**What to Benchmark**:
- Context creation time
- Sync protocol latency
- Cache hit rates
- Delta application throughput
- Memory usage under load

**Effort**: Low (2-3 days to set up criterion benchmarks)

---

### 15. **Dead Code / TODOs**
**Location**: Scattered

**Examples**:
```rust
// todo! potentially make this a dashmap::DashMap  <- DONE, used LRU instead
// todo! use cached::TimedSizedCache with a gc task  <- DONE

// objectives:  <- What are these??
//   keep up to N items, refresh entries as they are used
//   garbage collect entries as they expire, or as needed
```

**Action**: Clean up comments, remove obsolete TODOs.

**Effort**: Low (ongoing)

---

## 📝 Documentation Needs

### Missing Docs:
1. ✅ **Context Initialization Contract** - Added `INITIALIZATION_CONTRACT.md`
2. ✅ **Join Context Flow** - Added `JOIN_CONTEXT_ANALYSIS.md`
3. ✅ **Sync Protocol Security** - Added `PROTOCOL_SECURITY_ANALYSIS.md`
4. ✅ **Error Propagation Design** - Added `ERROR_PROPAGATION_ANALYSIS.md`
5. ✅ **Context Cache Design** - Added `CONTEXT_CACHE_DESIGN.md`
6. ❌ **Connection Pooling Design** - TODO
7. ❌ **Metrics & Observability Guide** - TODO
8. ❌ **Architecture Overview** - TODO
9. ❌ **Debugging Guide** - TODO

---

## 🎯 Recommended Next Steps

### Immediate (This Week):
1. ✅ Fix authentication protocol mismatch
2. ✅ Implement LRU cache for contexts
3. ⏭️ Add cache metrics (counter for hits/misses/evictions)
4. ⏭️ Apply LRU pattern to applications cache

### Short Term (This Month):
1. Implement connection pooling
2. Add request timeouts to all external calls
3. Set up basic metrics/observability
4. Add rate limiting for sync protocols

### Long Term (Next Quarter):
1. Evaluate actor pattern - consider simpler async fns
2. Implement graceful shutdown
3. Add integration test suite
4. Define domain-specific error types
5. Comprehensive benchmarking

---

## 📊 Metrics Wishlist

```rust
// What we should track
struct SystemMetrics {
    // Context Management
    context_cache_hits: Counter,
    context_cache_misses: Counter,
    context_cache_evictions: Counter,
    active_contexts: Gauge,
    context_creation_duration: Histogram,
    
    // Sync Protocols
    sync_requests_total: Counter,  // by (result: success/failure)
    sync_duration: Histogram,       // by (protocol: dag/full)
    sync_queue_depth: Gauge,
    sync_queue_full_errors: Counter,
    
    // Network
    active_connections: Gauge,      // by peer
    connection_pool_size: Gauge,
    connection_reuse_rate: Counter,
    bytes_sent: Counter,            // by (protocol: dag/blob/etc)
    bytes_received: Counter,
    
    // Delta Store
    delta_store_size: Gauge,        // by context
    dag_head_count: Histogram,
    delta_application_duration: Histogram,
    
    // Errors
    errors_total: Counter,          // by (component, error_type)
}
```

---

## 🏗️ Architecture Smells

### 1. **God Object: ContextManager**
- Manages contexts, applications, modules, external config
- 11 different message handlers
- Hundreds of lines of impl blocks

**Fix**: Split into focused components:
- `ContextRepository` (CRUD)
- `ApplicationManager` (CRUD)
- `ModuleLoader` (compilation)
- `ContextExecutor` (runtime)

### 2. **Tight Coupling**
- `ContextManager` depends on `NodeClient`, `ContextClient`, `ExternalClientConfig`
- Hard to test in isolation
- Circular dependencies (context ↔ node)

**Fix**: Dependency injection, trait-based abstractions.

### 3. **No Clear Boundaries**
- What's the difference between `context` and `context/primitives`?
- When do you use `NodeClient` vs `ContextClient`?
- What's `sync/manager.rs` vs `sync/delta_request.rs`?

**Fix**: Document module responsibilities, refactor for clarity.

---

## 💡 Quick Wins (< 1 day each)

1. ✅ Add LRU cache for contexts
2. ⏭️ Add LRU cache for applications
3. ⏭️ Add cache metrics (3 counters)
4. ⏭️ Add timeouts to external calls
5. ⏭️ Remove unused imports (many `std::mem`, `btree_map`)
6. ⏭️ Clean up obsolete TODOs
7. ⏭️ Make cache sizes configurable
8. ⏭️ Add structured logging to critical paths

---

## 🎬 Conclusion

The codebase has solid foundations but accumulated technical debt from rapid development. The critical issues (auth, memory leaks, error propagation) are now fixed.

**Priority Order**:
1. **Metrics & Observability** - Can't improve what you can't measure
2. **Connection Pooling** - Performance & resource usage
3. **Rate Limiting** - Security & stability
4. **Architecture Refactoring** - Long-term maintainability

The good news: Most issues are incremental improvements, not fundamental redesigns.

