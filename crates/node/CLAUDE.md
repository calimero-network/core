# calimero-node - Node Orchestration

Main node runtime that coordinates sync, storage, networking, and event handling.

- **Crate**: `calimero-node`
- **Entry**: `src/lib.rs`
- **Frameworks**: actix (actors), tokio (async)

## Build & Test

```bash
cargo build -p calimero-node
cargo test -p calimero-node
cargo test -p calimero-node test_sync -- --nocapture
cargo test -p calimero-node concurrent_branches -- --nocapture
cargo test -p calimero-node --test network_simulation
```

## File Layout

```
src/
├── lib.rs                     # NodeManager actor, NodeClients, NodeState
├── run.rs                     # start() function, NodeConfig
├── handlers/
│   ├── network_event.rs       # Network event handler
│   ├── state_delta.rs         # State delta handler
│   ├── stream_opened.rs       # Stream opened handler
│   ├── blob_protocol.rs       # Blob protocol handler
│   ├── get_blob_bytes.rs
│   └── specialized_node_invite.rs
├── sync/
│   ├── mod.rs                 # exception: mod.rs allowed here
│   ├── manager.rs             # SyncManager
│   ├── manager/application.rs
│   ├── stream.rs
│   ├── blobs.rs
│   ├── delta_request.rs
│   ├── snapshot.rs
│   └── tracking.rs
├── delta_store.rs
├── gc.rs
├── constants.rs
├── arbiter_pool.rs
└── utils.rs
primitives/src/
├── lib.rs                     # Shared types
├── client.rs                  # NodeClient
├── sync.rs                    # Sync types
└── messages/
```

## Key Components

### NodeManager Actor

```rust
// src/lib.rs
pub struct NodeManager {
    clients:  NodeClients,   // external service clients
    managers: NodeManagers,  // service managers
    state:    NodeState,     // runtime state
}

impl Actor for NodeManager {
    type Context = Context<Self>;
}
```

### Handler Pattern

```rust
// src/handlers/network_event.rs
impl Handler<NetworkEvent> for NodeManager {
    type Result = ();

    fn handle(&mut self, msg: NetworkEvent, ctx: &mut Self::Context) {
        // ...
    }
}
```

## Key Files

| File | Purpose |
|---|---|
| `src/lib.rs` | NodeManager actor |
| `src/run.rs` | `start()`, NodeConfig |
| `src/handlers/network_event.rs` | Network event handling |
| `src/handlers/state_delta.rs` | State delta processing |
| `src/sync/manager.rs` | Sync coordination |
| `primitives/src/client.rs` | NodeClient interface |

## Quick Search

```bash
rg -n "impl Handler" src/
rg -n "impl Message" src/
rg -n "pub async fn" src/sync/
rg -n "const " src/constants.rs
```

## Gotchas

- NodeManager is an actix Actor — use message passing, not direct calls
- Sync operations are async — always `await`
- Delta stores are keyed per `ContextId`
- `sync/mod.rs` is an intentional exception to the no-mod.rs rule
