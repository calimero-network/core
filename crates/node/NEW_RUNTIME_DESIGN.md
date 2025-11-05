# New Runtime Design - NO ACTORS!

## 🎯 Goal

Replace the current actor-based mess with clean async Rust runtime that:
- Uses `calimero-protocols` for all P2P communication
- Uses `calimero-sync` for orchestration
- NO actors (just tokio::spawn + channels)
- Simple, maintainable, testable

---

## 🏗️ Architecture Overview

```text
┌─────────────────────────────────────────────────────────────┐
│                    Node Runtime (main loop)                  │
│                                                              │
│  tokio::select! {                                           │
│    // Gossipsub broadcasts (state deltas)                   │
│    msg = gossipsub_rx.recv() => handle_broadcast(msg)       │
│                                                              │
│    // P2P request/response (delta request, blob, etc)       │
│    req = p2p_rx.recv() => handle_p2p_request(req)          │
│                                                              │
│    // Sync requests (from API or triggers)                  │
│    sync = sync_rx.recv() => handle_sync_request(sync)      │
│                                                              │
│    // Periodic tasks (heartbeat, cleanup)                   │
│    _ = heartbeat_ticker.tick() => run_heartbeat()          │
│  }                                                           │
└─────────────────────────────────────────────────────────────┘
                             ↓
        ┌────────────────────┴────────────────────┐
        │                                          │
        ↓                                          ↓
┌──────────────────┐                    ┌──────────────────┐
│  calimero-sync   │                    │ calimero-        │
│  (orchestration) │                    │ protocols        │
│                  │                    │ (stateless!)     │
│  - SyncScheduler │                    │                  │
│  - DagCatchup    │                    │  - SecureStream  │
│  - StateResync   │                    │  - key_exchange  │
└──────────────────┘                    │  - delta_request │
                                        │  - blob_request  │
                                        │  - state_delta   │
                                        └──────────────────┘
```

---

## 🔄 Event Loop Design

The runtime is a single async function with a `tokio::select!` loop:

```rust
pub async fn run_node_runtime(
    node_client: NodeClient,
    context_client: ContextClient,
    network_client: NetworkClient,
    config: NodeConfig,
) -> eyre::Result<()> {
    // Create channels
    let (gossipsub_tx, mut gossipsub_rx) = mpsc::unbounded_channel();
    let (p2p_tx, mut p2p_rx) = mpsc::unbounded_channel();
    let (sync_tx, mut sync_rx) = mpsc::channel(256);
    
    // Create sync scheduler
    let sync_scheduler = Arc::new(SyncScheduler::new(
        node_client.clone(),
        context_client.clone(),
        network_client.clone(),
        config.sync,
    ));
    
    // Spawn network listeners
    spawn_gossipsub_listener(gossipsub_tx);
    spawn_p2p_listener(p2p_tx);
    
    // Spawn periodic tasks
    let mut heartbeat = tokio::time::interval(Duration::from_secs(60));
    
    // Main event loop
    loop {
        tokio::select! {
            // Handle gossipsub broadcasts
            Some(msg) = gossipsub_rx.recv() => {
                handle_gossipsub_message(msg, &sync_scheduler).await?;
            }
            
            // Handle P2P requests
            Some(req) = p2p_rx.recv() => {
                handle_p2p_request(req, &sync_scheduler).await?;
            }
            
            // Handle sync requests
            Some(sync_req) = sync_rx.recv() => {
                handle_sync_request(sync_req, &sync_scheduler).await?;
            }
            
            // Periodic heartbeat
            _ = heartbeat.tick() => {
                run_heartbeat(&sync_scheduler).await?;
            }
        }
    }
}
```

---

## 📦 Message Types

### Gossipsub Message
```rust
enum GossipsubMessage {
    StateDelta {
        context_id: ContextId,
        author_id: PublicKey,
        delta_id: [u8; 32],
        // ... other fields
    },
}
```

### P2P Request
```rust
enum P2pRequest {
    DeltaRequest {
        stream: Stream,
        context_id: ContextId,
        delta_id: [u8; 32],
        their_identity: PublicKey,
    },
    BlobRequest {
        stream: Stream,
        context_id: ContextId,
        blob_id: BlobId,
        their_identity: PublicKey,
    },
    KeyExchange {
        stream: Stream,
        context: Context,
        their_identity: PublicKey,
    },
}
```

### Sync Request
```rust
struct SyncRequest {
    context_id: ContextId,
    peer_id: Option<PeerId>,
    result_tx: Option<oneshot::Sender<SyncResult>>,
}
```

---

## 🎨 Protocol Dispatch

**Old Way (Actors)**:
```rust
// Actor message passing
sync_manager.do_send(SyncMessage { context_id });
```

**New Way (Direct calls)**:
```rust
// Direct protocol calls
handle_delta_request(
    &mut stream,
    context_id,
    delta_id,
    their_identity,
    our_identity,
    &datastore_handle,
    delta_store.as_ref(),
    &context_client,
    timeout,
).await?;
```

---

## 📁 File Structure

```
crates/node/src/
├── runtime/                    NEW!
│   ├── mod.rs                 - Runtime exports
│   ├── event_loop.rs          - Main event loop
│   ├── dispatch.rs            - Protocol dispatch
│   ├── listeners.rs           - Network listeners
│   └── tasks.rs               - Periodic tasks
├── handlers/                   OLD (will migrate)
├── sync/                       OLD (being replaced)
└── lib.rs
```

---

## 🔌 Integration Points

### 1. Network Layer
- **Gossipsub**: Broadcast channel for state deltas
- **P2P Request/Response**: Stream-based protocols
- **Connection pooling**: Reuse connections

### 2. Storage Layer
- **DeltaStore**: In-memory DAG (implements trait from protocols)
- **ContextClient**: Database operations
- **BlobManager**: Blob storage

### 3. Execution Layer
- **ContextManager**: WASM execution (existing)
- **Event handlers**: Trigger on delta application

---

## 🧪 Testing Strategy

**Unit Tests**:
- Event loop logic
- Protocol dispatch
- Message routing

**Integration Tests**:
- Full sync flow
- Protocol interactions
- Error handling

**Property Tests** (future):
- Sync convergence
- DAG consistency
- No message loss

---

## 🚀 Migration Strategy

**Phase 1**: Build new runtime alongside old (THIS WEEK!)
- Create runtime/ directory
- Implement event loop
- Wire protocols + sync

**Phase 2**: Gradual migration (NEXT WEEK)
- Migrate one handler at a time
- Run both old + new in parallel
- Feature flag for rollback

**Phase 3**: Cleanup (WEEK AFTER)
- Delete old actor code
- Remove Actix dependencies
- Celebrate! 🎉

---

## 💡 Key Insights

1. **Simpler is better**: Event loop is easier than actors
2. **Composition over inheritance**: Use protocols like Lego bricks
3. **Explicit is better than implicit**: No magic message routing
4. **Testable by design**: No infrastructure needed

---

## 📊 Complexity Reduction

```
Old Runtime:
- SyncManager:        1,088 lines (actor)
- StateDeltaHandler:    765 lines (actor)
- Other handlers:       500+ lines (actors)
- Total:             2,353+ lines

New Runtime:
- event_loop.rs:       ~150 lines (async)
- dispatch.rs:         ~100 lines (async)
- listeners.rs:         ~80 lines (async)
- tasks.rs:             ~50 lines (async)
- Total:               ~380 lines (60% REDUCTION!)
```

---

## ✅ Success Criteria

1. ✅ No Actix actors
2. ✅ Uses calimero-protocols
3. ✅ Uses calimero-sync
4. ✅ Clean event loop
5. ✅ Testable
6. ✅ Compiles alongside old code
7. ✅ All e2e tests pass

---

Let's build it! 🚀

