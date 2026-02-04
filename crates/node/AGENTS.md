# calimero-node - Node Orchestration

Main node runtime that coordinates sync, storage, networking, and event handling.

## Package Identity

- **Crate**: `calimero-node`
- **Entry**: `src/lib.rs`
- **Framework**: actix (actors), tokio (async)

## Commands

```bash
# Build
cargo build -p calimero-node

# Test
cargo test -p calimero-node

# Test specific
cargo test -p calimero-node test_sync -- --nocapture
```

## File Organization

```
src/
├── lib.rs                    # NodeManager actor, NodeClients, NodeState
├── run.rs                    # Node startup (start function)
├── handlers.rs               # Handler module parent
├── handlers/
│   ├── network_event.rs      # Network event handler
│   ├── state_delta.rs        # State delta handler
│   ├── stream_opened.rs      # Stream opened handler
│   ├── blob_protocol.rs      # Blob protocol handler
│   ├── get_blob_bytes.rs     # Get blob bytes handler
│   └── specialized_node_invite.rs  # Specialized node invitation handler
├── sync/
│   ├── mod.rs                # Sync module (exception to no mod.rs rule)
│   ├── manager.rs            # SyncManager
│   ├── manager/
│   │   └── application.rs    # Application sync manager
│   ├── stream.rs             # Sync streams
│   ├── config.rs             # Sync configuration
│   ├── tracking.rs           # Sync tracking
│   ├── blobs.rs              # Blob sync
│   ├── delta_request.rs      # Delta request handling
│   ├── helpers.rs            # Sync helpers
│   ├── key.rs                # Sync key utilities
│   └── snapshot.rs           # Snapshot handling
├── delta_store.rs            # Delta storage
├── gc.rs                     # Garbage collection
├── constants.rs              # Constants
├── arbiter_pool.rs           # Actix arbiter pool
├── specialized_node_invite_state.rs  # Specialized node invite state
└── utils.rs                  # Utilities
primitives/                   # calimero-node-primitives
├── src/
│   ├── lib.rs                # Shared types
│   ├── client.rs             # NodeClient
│   ├── sync.rs               # Sync types
│   └── messages/             # Message types
```

## Key Components

### NodeManager Actor

Main coordinator using actix actor pattern:

```rust
// src/lib.rs
pub struct NodeManager {
    clients: NodeClients,      // External service clients
    managers: NodeManagers,    // Service managers
    state: NodeState,          // Runtime state
}

impl Actor for NodeManager {
    type Context = Context<Self>;
}
```

### Handler Pattern

- ✅ DO: Follow pattern in `src/handlers/network_event.rs`
- ✅ DO: Use actix message handlers

```rust
// src/handlers/network_event.rs
impl Handler<NetworkEvent> for NodeManager {
    type Result = ();

    fn handle(&mut self, msg: NetworkEvent, ctx: &mut Self::Context) -> Self::Result {
        // Handle network events
    }
}
```

### SyncManager

Handles state synchronization between nodes:

```rust
// src/sync/manager.rs
pub struct SyncManager {
    // Sync configuration and state
}
```

## Key Files

| File                            | Purpose                        |
| ------------------------------- | ------------------------------ |
| `src/lib.rs`                    | NodeManager actor definition   |
| `src/run.rs`                    | `start()` function, NodeConfig |
| `src/handlers/network_event.rs` | Network event handling         |
| `src/handlers/state_delta.rs`   | State delta processing         |
| `src/sync/manager.rs`           | Sync coordination              |
| `primitives/src/client.rs`      | NodeClient interface           |

## JIT Index

```bash
# Find handlers
rg -n "impl Handler" src/

# Find actor messages
rg -n "impl Message" src/

# Find sync logic
rg -n "pub async fn" src/sync/

# Find constants
rg -n "const " src/constants.rs
```

## Testing

```bash
# Run all node tests
cargo test -p calimero-node

# Run specific test
cargo test -p calimero-node concurrent_branches -- --nocapture

# Integration tests in tests/ directory
cargo test -p calimero-node --test network_simulation
```

## Common Gotchas

- NodeManager is an actix Actor - use message passing
- Sync operations are async - use proper await handling
- Delta stores are per-context (ContextId key)
