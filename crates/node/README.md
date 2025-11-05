# Calimero Node - P2P Runtime

> **Distributed node runtime for Calimero applications**

The node crate orchestrates WASM execution, state synchronization, and network communication for distributed Calimero applications. Clean architecture with extracted services and no actors.

---

## Quick Start

```rust
use calimero_node::{start, NodeConfig};

// Start a node
start(NodeConfig {
    network: network_config,
    sync: sync_config,
    datastore: store_config,
    blobstore: blob_config,
    context: context_config,
    server: server_config,
    gc_interval_secs: Some(43200), // 12 hours
}).await?;
```

**What you get:**
- ✅ **WASM execution** - run distributed applications
- ✅ **State sync** - automatic convergence across nodes
- ✅ **Event handlers** - reactive application logic
- ✅ **Blob sharing** - content-addressed file distribution
- ✅ **Clean architecture** - extracted services, no actors

---

## Architecture

```
Application Layer (WASM)
    ↓
┌───────────────────────────────────────────────────┐
│ Node Runtime (This Crate)                         │
│                                                   │
│  ┌─────────────┐  ┌──────────────────┐           │
│  │ Handlers    │  │ Services          │           │
│  │ • Network   │  │ • DeltaStore      │           │
│  │ • Streams   │  │ • BlobCache       │           │
│  │ • Events    │  │ • DeltaApplier    │           │
│  └─────────────┘  │ • TimerManager    │           │
│                   └──────────────────┘            │
│                                                   │
│  Delegates to:                                    │
│  • calimero-protocols (stateless handlers)        │
│  • calimero-sync (orchestration)                  │
│  • calimero-context (CRDT management)             │
└───────────────────────────────────────────────────┘
    ↓
┌───────────────────────────────────────────────────┐
│ Network Layer (libp2p)                            │
│ • Gossipsub broadcasts                            │
│ • P2P request/response                            │
│ • Peer discovery                                  │
└───────────────────────────────────────────────────┘
```

---

## Core Components

### DeltaStore

Manages DAG (Directed Acyclic Graph) of state changes per context.

```rust
use calimero_node::delta_store::DeltaStore;

// Create delta store for a context
let delta_store = DeltaStore::new(context_id, context_client, ...);

// Add delta (automatically handles cascade)
let applied = delta_store.add_delta(delta).await?;

// Query state
let heads = delta_store.heads().await;
let stats = delta_store.pending_stats().await;
```

**Responsibilities:**
- DAG validation (parent checking)
- Cascade application (pending → applied)
- State persistence
- Delta garbage collection

### Handlers

Event handlers dispatch to stateless protocols:

```rust
// Network event (gossipsub broadcast)
handle_network_event(event) {
    match event {
        StateDeltaBroadcast { ... } => {
            // Calls calimero_protocols::gossipsub::state_delta
            calimero_protocols::gossipsub::state_delta::handle_state_delta(...)
        }
        ...
    }
}

// Stream opened (P2P request)
handle_stream_opened(stream, protocol) {
    match protocol {
        CALIMERO_BLOB_PROTOCOL => {
            // Calls calimero_protocols::p2p::blob_protocol
            calimero_protocols::p2p::blob_protocol::handle_blob_protocol_stream(...)
        }
        ...
    }
}
```

**No business logic in handlers!** Just routing to protocols.

### Services

Extracted, focused services with single responsibilities:

| Service | Purpose | Lines |
|---------|---------|-------|
| **DeltaStoreService** | Manage delta stores per context | ~180 |
| **BlobCacheService** | LRU blob caching + eviction | ~140 |
| **DeltaApplier** | WASM delta application | ~100 |
| **TimerManager** | Periodic tasks (GC, heartbeat) | ~100 |

All services are simple, testable, and independent.

---

## State Synchronization

### How It Works

```
Node A creates delta → Gossipsub broadcast
    ↓
Node B receives → Validates parents
    ↓
Parents ready? 
├─ Yes → Apply immediately → Execute events
└─ No → Buffer as pending → Wait for parents
    ↓
Periodic sync fills gaps
    ↓
All nodes converge to same state ✅
```

### Dual-Path Sync

**Path 1: Gossipsub (Fast)**
- Broadcasts deltas to all peers
- ~100ms latency
- Handles 99% of normal operation

**Path 2: Periodic P2P (Recovery)**
- Uses `calimero-sync` orchestration
- Fills DAG gaps from packet loss
- Ensures eventual consistency

See [`calimero-sync` README](../sync/README.md) for details.

---

## Event Handling

When a delta contains events:

```rust
// Delta created with events
let delta = CausalDelta {
    id: delta_id,
    parents: dag_heads,
    payload: actions,
    events: Some(vec![MyEvent { ... }]),
    ...
};

// Applied on receiving node
delta_store.add_delta(delta).await?;

// If not author, execute handlers
if !is_author {
    for event in events {
        execute_handler(event).await?;
    }
}
```

**Critical rule:** Author nodes DO NOT execute their own handlers (prevents loops).

---

## Blob Management

Content-addressed file distribution:

```rust
// Store blob
let blob_id = blob_manager.put(data).await?;

// Retrieve blob (cached)
let data = blob_manager.get(&blob_id).await?;

// Automatically:
// - LRU caching (configurable max size)
// - Network fetching (if not local)
// - Periodic eviction (old blobs removed)
```

**Cache policy:**
- Max size: 1000 blobs
- Max age: 1 hour
- Max memory: 100MB

---

## Configuration

```rust
use calimero_node::NodeConfig;
use calimero_sync::SyncConfig;

NodeConfig {
    network: NetworkConfig {
        swarm: swarm_config,
        bootstrap: bootstrap_nodes,
        discovery: mdns_enabled,
    },
    
    sync: SyncConfig {
        max_concurrent_syncs: 10,
        retry_config: RetryConfig::default(),
        enable_heartbeat: false,
        heartbeat_interval: Duration::from_secs(30),
    },
    
    datastore: StoreConfig::new(path),
    blobstore: BlobStoreConfig::new(path),
    context: context_config,
    server: server_config,
    
    // Garbage collection interval (default: 12 hours)
    gc_interval_secs: Some(43200),
}
```

---

## Performance

### Latency

| Operation | Typical | Notes |
|-----------|---------|-------|
| **Local execution** | ~5-10ms | WASM + delta creation |
| **Gossipsub broadcast** | ~100ms | Network propagation |
| **Delta application** | ~1ms | DAG validation + apply |
| **Event handler** | ~5-10ms | WASM execution |
| **Periodic sync** | ~100-200ms | DAG catchup |

### Memory (per context)

| Component | Typical | Max |
|-----------|---------|-----|
| **DeltaStore (applied)** | ~5MB | ~10MB |
| **DeltaStore (pending)** | ~500KB | ~2MB |
| **BlobCache** | ~10MB | ~100MB |
| **Total** | ~15MB | ~110MB |

---

## Common Patterns

### Pattern 1: Execute Transaction

```rust
// Execute method on application
let outcome = context_manager.execute(
    &context_id,
    &method_name,
    args,
    &executor_id,
).await?;

// Automatically:
// - WASM execution
// - Delta creation
// - Gossipsub broadcast
// - Local application
// - Event emission
```

### Pattern 2: Join Context

```rust
// Join a context (triggers sync)
let (context_id, identity) = context_manager.join_context(
    invitation_payload,
).await?;

// Automatically:
// - Identity setup
// - Context subscription
// - Full state sync
// - Ready for execution ✅
```

### Pattern 3: Handle Events

```rust
// Define event handler in WASM
#[app::event(name = "item_added")]
pub fn on_item_added(event: ItemAdded) {
    // Update local state
    increment_counter(&event.user_id);
}

// Node runtime handles:
// - Event deserialization
// - Handler execution
// - Author check (skip if author)
// - Error handling
```

---

## Testing

```bash
# Unit tests
cargo test -p calimero-node --lib

# Integration tests (requires infrastructure)
cargo test -p calimero-node --test '*'

# With logs
RUST_LOG=debug cargo test -p calimero-node -- --nocapture
```

**Test coverage:**
- DeltaStore: DAG validation, cascade, cleanup
- BlobCache: LRU eviction, memory limits
- Services: Lifecycle, statistics, cleanup
- Handlers: Protocol dispatch, error handling

---

## Debugging

### Enable Detailed Logs

```bash
RUST_LOG=calimero_node=debug cargo run
```

### Common Issues

**Deltas stuck in pending:**
- Check `delta_store.pending_stats()`
- Verify network connectivity
- Check if parent deltas are missing
- Solution: Trigger manual sync

**Memory growing:**
- Check blob cache size
- Verify GC is running
- Inspect delta store stats
- Solution: Lower cache limits, increase GC frequency

**Sync failures:**
- Check sync events (SyncEvent)
- Verify peer connectivity
- Inspect retry attempts
- Solution: Check network, increase timeout

---

## API Reference

### DeltaStore

```rust
// Create store
DeltaStore::new(context_id, context_client, node_client, applier) -> Self

// Operations
async fn add_delta(&self, delta: CausalDelta) -> Result<bool>
async fn heads(&self) -> Result<Vec<Hash>>
async fn pending_stats(&self) -> Result<PendingStats>
async fn cleanup_stale(&self, max_age: Duration) -> Result<usize>
```

### BlobCacheService

```rust
// Create service
BlobCacheService::new() -> Self

// Operations
fn get(&self, blob_id: &BlobId) -> Option<Arc<[u8]>>
fn put(&self, blob_id: BlobId, data: Vec<u8>)
fn evict_old(&self) -> usize
fn stats(&self) -> CacheStats
```

### Services Module

```rust
// All services follow same pattern
mod services {
    pub mod blob_cache;      // LRU caching
    pub mod delta_store_service;  // Per-context stores
    pub mod delta_applier;   // WASM application
    pub mod timer_manager;   // Periodic tasks
}
```

---

## Design Principles

1. **Thin handlers** - Just routing, no logic
2. **Extracted services** - Single responsibility
3. **Stateless protocols** - All in calimero-protocols
4. **No actors** - Plain async Rust
5. **Observable** - Events for everything

---

## Migration from Old Architecture

The node crate was completely refactored:

**Removed (~13,000 lines):**
- ❌ Entire `sync/` directory (actors, managers)
- ❌ Handler wrappers and duplicates
- ❌ Tight coupling to Actix framework

**Added (clean architecture):**
- ✅ Services module (extracted responsibilities)
- ✅ Protocol delegation (calimero-protocols)
- ✅ Sync orchestration (calimero-sync)
- ✅ Clean handlers (routing only)

**Result:** Simpler, testable, maintainable! 🚀

---

## See Also

- [`calimero-protocols`](../protocols/README.md) - Network protocol handlers
- [`calimero-sync`](../sync/README.md) - Sync orchestration
- [`calimero-context`](../context/README.md) - Context management
- [`calimero-storage`](../storage/README.md) - CRDT collections

---

## License

See root [LICENSE](../../LICENSE) file.
