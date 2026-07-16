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
├── lib.rs                    # Crate root: module declarations and re-exports
├── manager.rs                # NodeManager actor
├── state.rs                  # NodeClients, NodeManagers, NodeState
├── run.rs                    # Node startup (start function)
├── handlers.rs               # Handler module parent
├── handlers/
│   ├── network_event.rs      # Network event handler
│   ├── network_event/
│   │   ├── namespace.rs      # ns/<id> topic dispatch (Op/Ack/ReadinessBeacon/ReadinessProbe)
│   │   └── readiness.rs      # ReadinessBeacon + ReadinessProbe receiver-side handlers
│   ├── state_delta/          # State delta handler (mod.rs, buffering.rs, crypto.rs, events.rs, store_setup.rs, verify.rs)
│   ├── stream_opened.rs      # Stream opened handler
│   ├── blob_protocol.rs      # Blob protocol handler
│   └── get_blob_bytes.rs     # Get blob bytes handler
├── readiness.rs              # ReadinessTier FSM + ReadinessCache + ReadinessManager actor
├── readiness/
│   └── tests.rs              # FSM transition tests + cache picker / atomicity tests
├── join_namespace.rs         # J6 namespace-join: join_namespace/await_namespace_ready/with_retry
├── sync/
│   ├── mod.rs                # Sync module (exception to no mod.rs rule)
│   ├── manager/              # SyncManager (mod.rs, blob_fetch.rs, handshake.rs, namespace_join.rs, namespace_sync.rs, tests.rs)
│   ├── stream.rs             # Sync streams
│   ├── config.rs             # Sync configuration
│   ├── tracking.rs           # Sync tracking
│   ├── blobs.rs              # Blob sync
│   ├── delta_request.rs      # Delta request handling
│   ├── helpers.rs            # Sync helpers
│   └── snapshot.rs           # Snapshot handling
├── delta_store.rs            # Delta storage
├── gc.rs                     # Garbage collection
├── constants.rs              # Constants
├── arbiter_pool.rs           # Actix arbiter pool
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
// src/manager.rs
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
// src/sync/manager/mod.rs
pub struct SyncManager {
    // Sync configuration and state
}
```

### ReadinessManager

Per-namespace readiness-beacon emitter and FSM driver. Closes the
cold-start gap between joining a namespace and being able to publish
governance ops without losing them to gossipsub's GRAFT handshake.
See #2237 and the `crates/node/src/readiness.rs` module doc.

```rust
// src/readiness.rs
pub struct ReadinessManager {
    pub cache: Arc<ReadinessCache>,           // shared with receiver
    pub config: ReadinessConfig,
    pub state_per_namespace: HashMap<[u8; 32], ReadinessState>,
    pub node_client: NodeClient,              // raw publish (bypasses 10s mesh-wait)
    pub datastore: Store,                     // namespace-identity loading
    pub last_probe_response_at: HashMap<(PeerId, [u8; 32]), Instant>,
}
```

- Beacons signed via `READINESS_BEACON_SIGN_DOMAIN` (canonical
  `signable_bytes`) and verified by
  `calimero_context::governance_broadcast::verify_readiness_beacon`
  (signature + namespace member set).
- Periodic emission on `beacon_interval` ticks for `*Ready` tiers.
- Edge-trigger emission on tier transition into `*Ready` (via
  `LocalStateChanged` / `ApplyBeaconLocal`).
- Probe-response rate-limit at `BEACON_INTERVAL / 2` per
  `(peer, namespace)` to close traffic + mailbox amplification.

### J6 namespace-join (Phase 8)

Free functions in `src/join_namespace.rs` (not on `ContextClient`
because of a Cargo dep cycle):

```rust
pub async fn join_namespace(...) -> Result<JoinStarted, JoinError>;
pub async fn await_namespace_ready(...) -> Result<ReadyReport, ReadyError>;
pub async fn join_and_wait_ready(...) -> Result<ReadyReport, ReadyError>;
pub async fn join_namespace_with_retry(...) -> Result<JoinStarted, JoinError>;
```

The fast path (`join_namespace`) provisions the namespace identity,
seeds local trust by writing a minimal `GroupMetaValue` with the
invitation's inviter as `admin_identity` (so beacons signed by the
inviter pass `verify_readiness_beacon`), subscribes to `ns/<id>`,
publishes a `ReadinessProbe`, and awaits the first fresh beacon.

## Key Files

| File                            | Purpose                        |
| ------------------------------- | ------------------------------ |
| `src/manager.rs`                | NodeManager actor definition   |
| `src/state.rs`                  | NodeClients, NodeManagers, NodeState |
| `src/run.rs`                    | `start()` function, NodeConfig |
| `src/handlers/network_event.rs` | Network event handling         |
| `src/handlers/network_event/namespace.rs` | `ns/<id>` topic dispatch (Op/Ack/Beacon/Probe) |
| `src/handlers/network_event/readiness.rs` | Beacon receive + probe forwarding |
| `src/handlers/state_delta/`     | State delta processing         |
| `src/readiness.rs`              | Readiness FSM + cache + manager (#2237) |
| `src/join_namespace.rs`         | J6 namespace-join flow         |
| `src/sync/manager/mod.rs`       | Sync coordination              |
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
- `ReadinessCache` and `ReadinessCacheNotify` use poison-recoverable
  mutex helpers (`entries_lock` / `waiters_lock`); never call `.lock()`
  directly on those fields
- `ReadinessCache::insert` does NOT verify signatures or membership -
  the receiver-side gate `verify_readiness_beacon` is the choke point;
  callers from outside the receiver path must verify first
- `ns/<id>` topic publishes wrap inner `NamespaceTopicMsg` in
  `BroadcastMessage::NamespaceGovernanceDelta { namespace_id, delta_id,
  parent_ids, payload: borsh(NamespaceTopicMsg) }` - sender-side
  envelope skips break receive-side decoding silently
