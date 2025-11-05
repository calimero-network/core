# Clean Architecture Design: 3-Crate Split

**Goal**: Replace Big Ball of Mud with clean, testable, reusable architecture.

**Approach**: Build new alongside old, migrate gradually, delete old.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│ calimero-protocols (CRATE 1)                                │
│ ─────────────────────────────────────────────────────────── │
│ Pure protocol handlers - NO state, NO dependencies          │
│                                                              │
│ pub async fn handle_state_delta(params) -> Result<()>       │
│ pub async fn handle_delta_request(params) -> Result<()>     │
│ pub async fn handle_blob_request(params) -> Result<()>      │
│ pub async fn handle_key_exchange(params) -> Result<()>      │
└─────────────────────────────────────────────────────────────┘
                          ▲
                          │ uses
┌─────────────────────────┴───────────────────────────────────┐
│ calimero-sync (CRATE 2)                                     │
│ ─────────────────────────────────────────────────────────── │
│ Sync orchestration - STATEFUL coordination                  │
│                                                              │
│ pub struct SyncScheduler { ... }                            │
│ pub async fn periodic_sync() -> Result<()>                  │
│ pub async fn dag_catchup() -> Result<()>                    │
└─────────────────────────────────────────────────────────────┘
                          ▲
                          │ uses
┌─────────────────────────┴───────────────────────────────────┐
│ calimero-node (CRATE 3)                                     │
│ ─────────────────────────────────────────────────────────── │
│ Thin runtime - wires protocols + sync together              │
│                                                              │
│ pub async fn run(config) -> Result<()> {                    │
│     tokio::spawn(timers);                                   │
│     handle_events(protocols, sync).await;                   │
│ }                                                            │
└─────────────────────────────────────────────────────────────┘
```

**Dependency Direction**: node → sync → protocols (no cycles!)

---

## Crate 1: calimero-protocols

### Purpose
Stateless network protocol handlers. Think of these as "pure functions" for network events.

### Structure
```
crates/protocols/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   │
│   ├── gossipsub/          # Broadcast protocols
│   │   ├── mod.rs
│   │   └── state_delta.rs  # Process state change broadcasts
│   │
│   ├── p2p/                # Request/response protocols
│   │   ├── mod.rs
│   │   ├── delta_request.rs   # Fetch specific delta
│   │   ├── blob_request.rs    # Fetch blob
│   │   └── key_exchange.rs    # Exchange encryption keys
│   │
│   └── stream/             # Shared stream utilities
│       ├── mod.rs
│       └── authenticated.rs   # ONE stream module (always secure)
```

### Key Principles

**1. Stateless** - Handlers take all state as parameters:
```rust
// GOOD: Stateless, testable
pub async fn handle_state_delta(
    delta: Delta,
    delta_store: &DeltaStore,        // Injected
    context_client: &ContextClient,  // Injected
    our_identity: PublicKey,         // Injected
) -> Result<ApplyResult> {
    // Pure logic, no hidden state
}

// BAD: Hidden state, hard to test
impl Handler {
    pub async fn handle_delta(&mut self) -> Result<()> {
        self.delta_store.add(...) // Where did this come from?
    }
}
```

**2. No dependencies on sync or runtime** - Only depends on:
- `calimero-dag` (DAG logic)
- `calimero-storage` (delta types)
- `calimero-crypto` (encryption)
- `calimero-network-primitives` (stream types)

**3. Always authenticated** - ONE stream API:
```rust
// stream/authenticated.rs
pub struct AuthenticatedStream {
    inner: Stream,
    shared_key: SharedKey,
    our_nonce: Nonce,
    their_nonce: Nonce,
}

impl AuthenticatedStream {
    /// Create from raw stream (performs authentication)
    pub async fn authenticate(
        stream: Stream,
        context: &Context,
        our_identity: PublicKey,
        context_client: &ContextClient,
    ) -> Result<Self>;
    
    pub async fn send(&mut self, message: &Message) -> Result<()>;
    pub async fn recv(&mut self) -> Result<Option<Message>>;
}

// NO WAY to send unauthenticated message (by design!)
```

---

## Crate 2: calimero-sync

### Purpose
Sync orchestration - coordinates when/how to sync with peers.

### Structure
```
crates/sync/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── scheduler.rs        # Periodic sync scheduling
│   ├── peer_selector.rs    # Select which peer to sync with
│   │
│   └── strategies/         # Sync strategies
│       ├── mod.rs
│       ├── dag_catchup.rs  # Delta-based incremental sync
│       └── state_resync.rs # Full state resync (fallback)
```

### Key Principles

**1. Stateful coordination** - Owns sync state:
```rust
pub struct SyncScheduler {
    last_sync: HashMap<ContextId, Instant>,
    min_interval: Duration,
    contexts: Vec<ContextId>,
}

impl SyncScheduler {
    pub fn should_sync(&mut self, context_id: &ContextId) -> bool {
        // Stateful decision based on last_sync
    }
    
    pub async fn sync_context(
        &mut self,
        context_id: &ContextId,
        peer: PeerId,
    ) -> Result<SyncResult> {
        // Delegates to strategies, updates state
    }
}
```

**2. Uses protocols** - Calls protocol handlers:
```rust
// In dag_catchup.rs
pub async fn sync_dag(
    context_id: &ContextId,
    peer: PeerId,
    our_heads: Vec<Hash>,
) -> Result<SyncResult> {
    // 1. Request peer's DAG heads
    let their_heads = protocols::p2p::request_dag_heads(peer, context_id).await?;
    
    // 2. Find missing deltas
    let missing = find_missing(our_heads, their_heads);
    
    // 3. Request each missing delta
    for delta_id in missing {
        protocols::p2p::request_delta(peer, context_id, delta_id).await?;
    }
    
    Ok(SyncResult::DagCatchup)
}
```

**3. No network code** - Just orchestration logic

---

## Crate 3: calimero-node

### Purpose
Thin runtime - wires everything together and manages resources.

### Structure
```
crates/node/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Public API: start()
│   ├── runtime.rs          # Main event loop (NO actors!)
│   │
│   ├── resources/          # Local resources
│   │   ├── blob_cache.rs   # LRU blob cache
│   │   └── delta_stores.rs # Per-context DAG stores
│   │
│   └── tasks/              # Background tasks
│       ├── timers.rs       # Spawn periodic tasks
│       ├── blob_eviction.rs
│       ├── delta_cleanup.rs
│       └── heartbeat.rs
```

### Key Principles

**1. No actors** - Just async functions:
```rust
// runtime.rs
pub async fn run(config: NodeConfig) -> Result<()> {
    let state = Arc::new(Mutex::new(NodeState::new()));
    let sync_scheduler = Arc::new(Mutex::new(SyncScheduler::new()));
    
    // Spawn periodic tasks
    tokio::spawn(tasks::blob_eviction_task(state.clone()));
    tokio::spawn(tasks::delta_cleanup_task(state.clone()));
    tokio::spawn(tasks::heartbeat_task(state.clone()));
    tokio::spawn(tasks::periodic_sync_task(sync_scheduler.clone()));
    
    // Main event loop
    loop {
        tokio::select! {
            // Gossipsub broadcast received
            Some(delta) = gossipsub_rx.recv() => {
                let state = state.lock().await;
                protocols::gossipsub::handle_state_delta(
                    delta,
                    &state.delta_stores,
                    &context_client,
                ).await?;
            }
            
            // P2P stream opened
            Some((peer, stream)) = stream_rx.recv() => {
                tokio::spawn(handle_p2p_stream(peer, stream, state.clone()));
            }
            
            // Graceful shutdown
            _ = shutdown_rx.recv() => {
                break;
            }
        }
    }
    
    Ok(())
}
```

**2. Thin layer** - Most logic in protocols + sync:
```rust
async fn handle_p2p_stream(
    peer: PeerId,
    mut stream: Stream,
    state: Arc<Mutex<NodeState>>,
) -> Result<()> {
    // Authenticate
    let mut auth_stream = AuthenticatedStream::authenticate(
        stream,
        &context,
        our_identity,
        &context_client,
    ).await?;
    
    // Read request type
    let request = auth_stream.recv().await?;
    
    // Delegate to protocol handler
    match request {
        Request::Delta { delta_id } => {
            protocols::p2p::handle_delta_request(
                &mut auth_stream,
                delta_id,
                &state.lock().await.delta_stores,
            ).await?;
        }
        Request::Blob { blob_id } => {
            protocols::p2p::handle_blob_request(
                &mut auth_stream,
                blob_id,
                &state.lock().await.blob_cache,
            ).await?;
        }
        // ...
    }
    
    Ok(())
}
```

**3. Resource management only** - Caching, eviction, GC

---

## Migration Strategy

### Phase 1: Create calimero-protocols (Week 1)

**Tasks**:
1. Create `crates/protocols/` (new crate)
2. Copy protocol handlers from current node:
   - `state_delta.rs` → `protocols/gossipsub/state_delta.rs`
   - `sync/delta_request.rs` → `protocols/p2p/delta_request.rs`
   - `sync/blobs.rs` → `protocols/p2p/blob_request.rs`
   - `sync/key.rs` → `protocols/p2p/key_exchange.rs`
3. Merge `stream.rs` + `secure_stream.rs` → `protocols/stream/authenticated.rs`
4. Make all handlers stateless (take state as params)
5. Add tests (protocols now testable without full node!)

**Benefits**:
- ✅ Protocols isolated and testable
- ✅ ONE stream module (always secure)
- ✅ Can reuse in different contexts

**Compatibility**: Keep old code, just copy (no breaking changes)

### Phase 2: Create calimero-sync (Week 2)

**Tasks**:
1. Create `crates/sync/` (new crate)
2. Move sync orchestration from current node:
   - `sync/manager.rs` → `sync/scheduler.rs` (rename for clarity)
   - Extract DAG catchup logic → `sync/strategies/dag_catchup.rs`
   - Extract state resync logic → `sync/strategies/state_resync.rs`
3. Remove actor dependencies (just plain structs + async fns)
4. Use protocols crate for network operations
5. Add tests (sync logic now testable!)

**Benefits**:
- ✅ Sync logic isolated
- ✅ No actors (simpler)
- ✅ Uses clean protocol APIs

**Compatibility**: Keep old code (no breaking changes)

### Phase 3: Create new calimero-node runtime (Week 3)

**Tasks**:
1. Create `src/runtime.rs` in calimero-node
2. Implement main event loop (tokio::select!, NO actors)
3. Spawn periodic tasks (tokio::spawn, NO run_interval)
4. Wire protocols + sync together
5. Add tests

**Benefits**:
- ✅ Simple event loop (no framework magic)
- ✅ Easy to understand (plain Rust async)
- ✅ Easy to debug (no actor indirection)

**Compatibility**: New entry point `run_v2()`, keep old `start()`

### Phase 4: Migrate & Delete (Week 4)

**Tasks**:
1. Update server/CLI to use `run_v2()`
2. Run E2E tests with new runtime
3. Fix any issues
4. Delete old code when stable
5. Rename `run_v2()` → `run()`

---

## Detailed Design: calimero-protocols

### Crate Structure

```toml
# crates/protocols/Cargo.toml
[package]
name = "calimero-protocols"
version = "0.1.0"

[dependencies]
calimero-dag = { path = "../dag" }
calimero-storage = { path = "../storage" }
calimero-crypto = { path = "../crypto" }
calimero-network-primitives = { path = "../network/primitives" }
calimero-context-primitives = { path = "../context/primitives" }

eyre = "0.6"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
borsh = "1"
serde_json = "1"

# NO actix dependency!
# NO sync logic!
```

### Public API

```rust
// crates/protocols/src/lib.rs

pub mod gossipsub;
pub mod p2p;
pub mod stream;

// Re-exports
pub use stream::AuthenticatedStream;

// Gossipsub protocols
pub use gossipsub::state_delta::handle_state_delta;

// P2P protocols
pub use p2p::delta_request::{handle_delta_request, request_delta};
pub use p2p::blob_request::{handle_blob_request, request_blob};
pub use p2p::key_exchange::{handle_key_exchange, request_key_exchange};
```

### Example: Gossipsub State Delta Handler

```rust
// crates/protocols/src/gossipsub/state_delta.rs

use calimero_dag::{CausalDelta, DeltaStore};
use calimero_storage::action::Action;
use eyre::Result;

/// Parameters for processing a state delta broadcast
pub struct StateDeltaParams<'a> {
    pub delta_id: [u8; 32],
    pub parents: Vec<[u8; 32]>,
    pub artifact: Vec<u8>,        // Encrypted actions
    pub author_id: PublicKey,
    pub sender_key: PrivateKey,   // For decryption
    pub nonce: Nonce,
    pub expected_root_hash: [u8; 32],
    pub events: Option<Vec<u8>>,
    
    // Injected dependencies (NO hidden state!)
    pub delta_store: &'a DeltaStore,
    pub context_client: &'a ContextClient,
    pub our_identity: PublicKey,
}

/// Result of processing a state delta
pub struct StateDeltaResult {
    pub applied: bool,              // Was delta applied immediately?
    pub cascaded: Vec<[u8; 32]>,    // IDs of cascaded deltas
    pub events: Option<Vec<ExecutionEvent>>,
}

/// Process a state delta broadcast (gossipsub).
///
/// This is a PURE FUNCTION - no hidden state, fully testable.
pub async fn handle_state_delta(params: StateDeltaParams<'_>) -> Result<StateDeltaResult> {
    // 1. Decrypt artifact
    let shared_key = SharedKey::from_sk(&params.sender_key.into());
    let decrypted = shared_key.decrypt(params.artifact, params.nonce)
        .ok_or_eyre("decryption failed")?;
    
    // 2. Deserialize actions
    let storage_delta: StorageDelta = borsh::from_slice(&decrypted)?;
    let actions = match storage_delta {
        StorageDelta::Actions(actions) => actions,
        _ => bail!("expected Actions variant"),
    };
    
    // 3. Create DAG delta
    let delta = CausalDelta {
        id: params.delta_id,
        parents: params.parents,
        payload: actions,
        hlc: /* ... */,
        expected_root_hash: params.expected_root_hash,
    };
    
    // 4. Add to DAG (injected, not owned!)
    let result = params.delta_store.add_delta(delta).await?;
    
    // 5. Parse events if delta was applied
    let events = if result.applied {
        params.events.as_ref()
            .map(|data| serde_json::from_slice(data))
            .transpose()?
    } else {
        None
    };
    
    Ok(StateDeltaResult {
        applied: result.applied,
        cascaded: result.cascaded_ids,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_handle_state_delta_success() {
        // Create mock delta_store
        let delta_store = MockDeltaStore::new();
        
        // Create test delta
        let params = StateDeltaParams {
            delta_id: [1; 32],
            // ... all parameters explicit!
            delta_store: &delta_store,
        };
        
        // Test pure function (no hidden state!)
        let result = handle_state_delta(params).await.unwrap();
        
        assert!(result.applied);
    }
}
```

**Notice**:
- ✅ Pure function (all inputs explicit)
- ✅ No hidden state (delta_store injected)
- ✅ Testable (can mock delta_store)
- ✅ No actors, no framework magic
- ✅ Clear inputs and outputs

---

### Example: P2P Delta Request Handler

```rust
// crates/protocols/src/p2p/delta_request.rs

/// Handle incoming delta request from peer (server side).
pub async fn handle_delta_request(
    stream: &mut AuthenticatedStream,  // Already authenticated!
    delta_id: [u8; 32],
    delta_store: &DeltaStore,
) -> Result<()> {
    // Load delta from store
    let Some(delta) = delta_store.get_delta(&delta_id).await else {
        stream.send(&Response::DeltaNotFound).await?;
        return Ok(());
    };
    
    // Serialize and send
    let serialized = borsh::to_vec(&delta)?;
    stream.send(&Response::Delta(serialized)).await?;
    
    Ok(())
}

/// Request a delta from peer (client side).
pub async fn request_delta(
    peer: PeerId,
    context_id: &ContextId,
    delta_id: [u8; 32],
    network_client: &NetworkClient,
    context_client: &ContextClient,
) -> Result<CausalDelta<Vec<Action>>> {
    // Open stream
    let stream = network_client.open_stream(peer).await?;
    
    // Authenticate
    let mut auth_stream = AuthenticatedStream::authenticate(
        stream,
        context_id,
        our_identity,
        context_client,
    ).await?;
    
    // Send request
    auth_stream.send(&Request::Delta { delta_id }).await?;
    
    // Receive response
    let response = auth_stream.recv().await?;
    
    match response {
        Response::Delta(data) => {
            let delta = borsh::from_slice(&data)?;
            Ok(delta)
        }
        Response::DeltaNotFound => {
            bail!("peer doesn't have delta");
        }
    }
}
```

**Notice**:
- ✅ Authentication is mandatory (AuthenticatedStream type)
- ✅ Request and handler in same file (clear protocol)
- ✅ Stateless (delta_store injected)
- ✅ Simple async fn (no actors)

---

## Detailed Design: calimero-sync

### Sync Scheduler

```rust
// crates/sync/src/scheduler.rs

pub struct SyncScheduler {
    // When we last synced each context
    last_sync: HashMap<ContextId, Instant>,
    
    // Minimum time between syncs for same context
    min_interval: Duration,
    
    // Strategy picker (tries cheap strategies first)
    strategy: Box<dyn SyncStrategy>,
}

impl SyncScheduler {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            last_sync: HashMap::new(),
            min_interval,
            strategy: Box::new(strategies::SmartStrategy::new()),
        }
    }
    
    /// Check if context should sync now
    pub fn should_sync(&mut self, context_id: &ContextId) -> bool {
        match self.last_sync.get(context_id) {
            None => true, // Never synced
            Some(&last) => {
                let elapsed = Instant::now().duration_since(last);
                elapsed >= self.min_interval
            }
        }
    }
    
    /// Sync a context with a peer
    pub async fn sync(
        &mut self,
        context_id: &ContextId,
        peer: PeerId,
        delta_store: &DeltaStore,
        network_client: &NetworkClient,
        context_client: &ContextClient,
    ) -> Result<SyncResult> {
        // Try strategy
        let result = self.strategy.sync(
            context_id,
            peer,
            delta_store,
            network_client,
            context_client,
        ).await?;
        
        // Update last_sync
        self.last_sync.insert(*context_id, Instant::now());
        
        Ok(result)
    }
}
```

### Sync Strategies

```rust
// crates/sync/src/strategies/mod.rs

pub trait SyncStrategy: Send + Sync {
    async fn sync(
        &self,
        context_id: &ContextId,
        peer: PeerId,
        delta_store: &DeltaStore,
        network_client: &NetworkClient,
        context_client: &ContextClient,
    ) -> Result<SyncResult>;
}

// Smart strategy: Try DAG catchup first, fallback to full resync
pub struct SmartStrategy {
    dag_catchup: DagCatchupStrategy,
    state_resync: StateResyncStrategy,
}

impl SyncStrategy for SmartStrategy {
    async fn sync(&self, ...) -> Result<SyncResult> {
        // Try cheap DAG catchup first
        match self.dag_catchup.sync(...).await {
            Ok(result) => Ok(result),
            Err(_) => {
                // Fallback to expensive full resync
                self.state_resync.sync(...).await
            }
        }
    }
}
```

---

## Detailed Design: calimero-node (Runtime)

### Main Runtime

```rust
// crates/node/src/runtime.rs

pub struct NodeRuntime {
    // Resources
    blob_cache: Arc<BlobCacheService>,
    delta_stores: Arc<DeltaStoreService>,
    
    // Clients
    context_client: ContextClient,
    node_client: NodeClient,
    network_client: NetworkClient,
    
    // Sync
    sync_scheduler: Arc<Mutex<SyncScheduler>>,
}

impl NodeRuntime {
    pub async fn run(self) -> Result<()> {
        // Spawn background tasks
        self.spawn_tasks();
        
        // Main event loop (NO actors!)
        self.event_loop().await
    }
    
    fn spawn_tasks(&self) {
        // Blob eviction (every 5 min)
        tokio::spawn(tasks::blob_eviction(
            self.blob_cache.clone(),
            Duration::from_secs(300),
        ));
        
        // Delta cleanup (every 60 sec)
        tokio::spawn(tasks::delta_cleanup(
            self.delta_stores.clone(),
            Duration::from_secs(60),
        ));
        
        // Heartbeat (every 30 sec)
        tokio::spawn(tasks::heartbeat(
            self.context_client.clone(),
            self.node_client.clone(),
            Duration::from_secs(30),
        ));
        
        // Periodic sync (every 10 sec)
        tokio::spawn(tasks::periodic_sync(
            self.sync_scheduler.clone(),
            self.delta_stores.clone(),
            self.context_client.clone(),
            self.network_client.clone(),
            Duration::from_secs(10),
        ));
    }
    
    async fn event_loop(&self) -> Result<()> {
        loop {
            tokio::select! {
                // Gossipsub broadcast
                Some(event) = self.node_client.recv_broadcast() => {
                    self.handle_broadcast(event).await?;
                }
                
                // P2P stream opened
                Some((peer, stream)) = self.network_client.recv_stream() => {
                    let runtime = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = runtime.handle_p2p_stream(peer, stream).await {
                            warn!(?e, %peer, "P2P stream failed");
                        }
                    });
                }
                
                // Shutdown signal
                _ = self.shutdown_rx.recv() => {
                    info!("Shutting down node runtime");
                    break;
                }
            }
        }
        
        Ok(())
    }
    
    async fn handle_broadcast(&self, event: BroadcastEvent) -> Result<()> {
        match event {
            BroadcastEvent::StateDelta { context_id, author_id, delta, ... } => {
                // Delegate to protocol handler (stateless!)
                let params = protocols::gossipsub::StateDeltaParams {
                    delta_id: delta.id,
                    // ... all params explicit
                    delta_store: &self.delta_stores.get_or_create(&context_id),
                    context_client: &self.context_client,
                };
                
                let result = protocols::gossipsub::handle_state_delta(params).await?;
                
                // Execute event handlers if delta was applied
                if result.applied && result.events.is_some() {
                    // ...
                }
                
                Ok(())
            }
        }
    }
    
    async fn handle_p2p_stream(&self, peer: PeerId, stream: Stream) -> Result<()> {
        // Read request type from stream init
        let request = read_init_message(stream).await?;
        
        // Authenticate based on context + identity
        let mut auth_stream = protocols::stream::AuthenticatedStream::authenticate(
            stream,
            &request.context_id,
            request.party_id,
            &self.context_client,
        ).await?;
        
        // Route to protocol handler
        match request.payload {
            InitPayload::DeltaRequest { context_id, delta_id } => {
                protocols::p2p::handle_delta_request(
                    &mut auth_stream,
                    delta_id,
                    &self.delta_stores.get(&context_id),
                ).await?;
            }
            InitPayload::BlobShare { blob_id } => {
                protocols::p2p::handle_blob_request(
                    &mut auth_stream,
                    blob_id,
                    &self.blob_cache,
                ).await?;
            }
            InitPayload::KeyShare => {
                protocols::p2p::handle_key_exchange(
                    &mut auth_stream,
                    &self.context_client,
                ).await?;
            }
        }
        
        Ok(())
    }
}
```

**Notice**:
- ✅ NO actors (plain struct + async methods)
- ✅ Clear event loop (tokio::select!)
- ✅ Protocols are stateless (state injected)
- ✅ Simple to understand (no framework magic)

---

## Comparison: Before vs After

### Before (Current)

```
calimero-node/
├── lib.rs (240 lines) ← NodeManager actor
├── handlers/
│   ├── network_event.rs ← Actix Handler
│   ├── state_delta.rs (765 lines!) ← Giant function
│   ├── stream_opened.rs ← Routes streams
│   └── blob_protocol.rs ← P2P handler
├── sync/
│   ├── manager.rs (1088 lines!) ← Giant manager
│   ├── delta_request.rs ← P2P protocol
│   ├── blobs.rs ← P2P protocol
│   ├── key.rs ← P2P protocol
│   ├── stream.rs ← Insecure helper
│   └── secure_stream.rs ← Secure helper
└── ... 22 files, 6274 lines total
```

**Problems**:
- 🔴 Everything in one crate (tight coupling)
- 🔴 Actors everywhere (unnecessary complexity)
- 🔴 Giant functions (state_delta: 765 lines!)
- 🔴 Confusing structure (handlers vs sync?)
- 🔴 Two stream modules (insecure by default)

### After (Clean)

```
calimero-protocols/
├── gossipsub/
│   └── state_delta.rs (~200 lines) ← Pure function
├── p2p/
│   ├── delta_request.rs (~100 lines) ← Request + handler
│   ├── blob_request.rs (~100 lines)
│   └── key_exchange.rs (~100 lines)
└── stream/
    └── authenticated.rs (~300 lines) ← ONE stream module

calimero-sync/
├── scheduler.rs (~200 lines) ← Periodic sync
└── strategies/
    ├── dag_catchup.rs (~150 lines)
    └── state_resync.rs (~150 lines)

calimero-node/
├── runtime.rs (~300 lines) ← Main loop (NO actors!)
├── resources/
│   ├── blob_cache.rs
│   └── delta_stores.rs
└── tasks/
    └── timers.rs (~100 lines) ← Spawn periodic tasks
```

**Benefits**:
- ✅ 3 focused crates (clear boundaries)
- ✅ NO actors (plain async Rust)
- ✅ Small files (~100-300 lines each)
- ✅ Clear structure (protocols vs sync vs runtime)
- ✅ Secure by default (ONE stream module, always authenticated)
- ✅ Testable (protocols test without full node)
- ✅ Reusable (protocols can be used elsewhere)

**Total lines**: ~2000 (vs 6274 current) - 68% reduction!

---

## Decision Points

### A. Keep or Kill Actors?

**My Recommendation**: KILL

**Rationale**:
- We don't use supervision
- We don't use actor hierarchy
- run_interval() → tokio::spawn + interval (same thing!)
- Handler<Message> → async fn (simpler!)

**Your Call**: Do you see any value in keeping actors?

### B. Split into 3 crates or keep 1?

**My Recommendation**: SPLIT

**Rationale**:
- Protocols reusable elsewhere
- Sync testable in isolation
- Clear boundaries (forces good design)

**Your Call**: Worth the extra crates?

### C. Implement alongside or rewrite in-place?

**My Recommendation**: ALONGSIDE

**Rationale**:
- Low risk (old code still works)
- Can test new thoroughly before switching
- Easy rollback if issues

**Your Call**: Or just rip out old and rewrite?

---

## Timeline Estimate

**Week 1**: calimero-protocols
- Extract + clean protocol handlers
- Merge stream.rs + secure_stream.rs
- Add tests
- **Deliverable**: Protocols crate (testable, reusable)

**Week 2**: calimero-sync
- Extract sync orchestration
- Remove actors (plain async)
- Add tests
- **Deliverable**: Sync crate (testable, clean)

**Week 3**: calimero-node runtime
- New runtime.rs (NO actors!)
- Spawn tasks with tokio::spawn
- Wire protocols + sync
- **Deliverable**: New runtime (simpler, clearer)

**Week 4**: Migration
- Update server to use new runtime
- Run E2E tests
- Delete old code
- **Deliverable**: Clean architecture, old code deleted

**Total**: 4 weeks for complete rewrite

---

## Next Step

**I need your approval on**:
1. Kill actors? (yes/no)
2. Split into 3 crates? (yes/no)
3. Implement alongside old? (yes/no)

Then I'll start implementing calimero-protocols.

**Ready to proceed?**

