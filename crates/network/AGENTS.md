# calimero-network - P2P Networking

Peer-to-peer networking layer using libp2p for peer discovery, gossipsub messaging, and direct streams.

## Package Identity

- **Crate**: `calimero-network`
- **Entry**: `src/lib.rs`
- **Framework**: libp2p (P2P), tokio (async), actix (actors)
- **Related Docs**: [ARCHITECTURE.md](ARCHITECTURE.md), [PROTOCOLS.md](PROTOCOLS.md), [README.md](README.md)

## Commands

```bash
# Build
cargo build -p calimero-network

# Test
cargo test -p calimero-network

# Test with Kad modes
cargo test -p calimero-network --test kad_modes
```

## Architecture Overview

### High-Level Integration

```text
┌─────────────────────────────────────────────────────────────────────┐
│                        calimero-node                                 │
│                                                                      │
│  ┌──────────────┐    ┌──────────────┐    ┌───────────────────────┐  │
│  │ NodeManager  │◄───│ SyncManager  │◄───│ Network Event Handler │  │
│  │   (actor)    │    │              │    │                       │  │
│  └──────┬───────┘    └──────┬───────┘    └───────────┬───────────┘  │
│         │                   │                        │              │
│         ▼                   ▼                        │              │
│  ┌──────────────┐    ┌──────────────┐               │              │
│  │   Storage    │    │     DAG      │               │              │
│  │   (CRDTs)    │    │   (deltas)   │               │              │
│  └──────────────┘    └──────────────┘               │              │
└─────────────────────────────────────────────────────┼──────────────┘
                                                      │
                 NetworkMessage (commands)            │ NetworkEvent (events)
                          │                           │
                          ▼                           │
┌─────────────────────────────────────────────────────┴──────────────┐
│                       calimero-network                              │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │                     NetworkManager (actor)                   │   │
│  │  ┌───────────────┐  ┌───────────────┐  ┌─────────────────┐  │   │
│  │  │ Swarm         │  │ Discovery     │  │ Event           │  │   │
│  │  │ (libp2p)      │  │ State         │  │ Dispatcher      │  │   │
│  │  └───────┬───────┘  └───────────────┘  └─────────────────┘  │   │
│  └──────────┼──────────────────────────────────────────────────┘   │
│             │                                                       │
│             ▼                                                       │
│       Behaviour (11 sub-behaviours: gossipsub, kad, mdns, etc.)     │
│       See ARCHITECTURE.md#behaviour-composition for details         │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
                        P2P Network
```

### Data Flow: State Delta Broadcast

```text
1. App writes to CRDT (calimero-storage)
                │
                ▼
2. Delta generated, added to DAG (calimero-dag)
                │
                ▼
3. NodeManager broadcasts via NetworkClient
   network_client.publish(context_topic, delta_bytes).await
                │
                ▼
4. NetworkManager.handle(Publish{...})
   swarm.behaviour_mut().gossipsub.publish(topic, data)
                │
                ▼
5. libp2p Gossipsub broadcasts to mesh peers
                │
                ▼
6. Remote node: Gossipsub.Event::Message received
                │
                ▼
7. EventHandler dispatches NetworkEvent::Message
   event_dispatcher.dispatch(NetworkEvent::Message{...})
                │
                ▼
8. NodeManager receives, applies delta to local CRDT
```

### Data Flow: Sync Stream

```text
1. Node A detects missing state (DAG heads differ from peer)
                │
                ▼
2. SyncManager requests stream via NetworkClient
   network_client.open_stream(peer_id).await
                │
                ▼
3. NetworkManager opens libp2p stream
   swarm.behaviour().stream.new_control().open_stream(peer_id, protocol)
                │
                ▼
4. Stream established, NetworkEvent::StreamOpened emitted
                │
                ▼
5. Both sides run sync protocol over stream:
   - Hash comparison (tree traversal)
   - Delta requests/responses
   - Entity transfers
                │
                ▼
6. Stream closed, state synchronized
```

## File Organization

```
src/
├── lib.rs                    # NetworkManager actor, public exports
├── behaviour.rs              # Composed Behaviour (11 sub-behaviours)
├── discovery.rs              # Discovery coordination
├── discovery/
│   ├── state.rs              # DiscoveryState (peer tracking, reachability)
│   └── state_tests.rs        # Discovery state tests
├── handlers.rs               # Handler module exports
├── handlers/
│   ├── commands.rs           # NetworkMessage handler (dispatch)
│   ├── commands/
│   │   ├── subscribe.rs      # Gossipsub subscription
│   │   ├── unsubscribe.rs    # Gossipsub unsubscription
│   │   ├── publish.rs        # Gossipsub publish
│   │   ├── open_stream.rs    # Open direct stream
│   │   ├── dial.rs           # Dial peer
│   │   ├── listen.rs         # Listen on address
│   │   ├── bootstrap.rs      # Bootstrap DHT
│   │   ├── peer_count.rs     # Get connected peer count
│   │   ├── mesh_peer_count.rs # Get mesh peer count for topic
│   │   ├── mesh_peers.rs     # Get mesh peers for topic
│   │   ├── announce_blob.rs  # Announce blob availability (DHT)
│   │   ├── query_blob.rs     # Query blob providers (DHT)
│   │   ├── request_blob.rs   # Request blob from peer
│   │   ├── send_specialized_node_invitation_response.rs
│   │   └── send_specialized_node_verification_request.rs
│   ├── stream.rs             # Stream handler exports
│   └── stream/
│       ├── incoming.rs       # Incoming stream handler
│       ├── rendezvous.rs     # Rendezvous tick handler
│       ├── swarm.rs          # SwarmEvent handler (main event loop)
│       └── swarm/
│           ├── gossipsub.rs  # Gossipsub events → NetworkEvent
│           ├── mdns.rs       # mDNS discovery events
│           ├── kad.rs        # Kademlia DHT events
│           ├── ping.rs       # Ping protocol events
│           ├── identify.rs   # Identify protocol events
│           ├── autonat.rs    # AutoNAT events
│           ├── dcutr.rs      # DCUtR (hole punching) events
│           ├── relay.rs      # Relay events
│           ├── rendezvous.rs # Rendezvous events
│           └── specialized_node_invite.rs
primitives/                   # calimero-network-primitives
└── src/
    ├── lib.rs                # Module exports
    ├── config.rs             # NetworkConfig, DiscoveryConfig, etc.
    ├── messages.rs           # NetworkMessage, NetworkEvent, dispatcher trait
    ├── client.rs             # NetworkClient (async API)
    ├── stream.rs             # Stream wrapper, protocols
    ├── stream/
    │   └── codec.rs          # MessageCodec (length-delimited framing)
    ├── blob_types.rs         # Blob-related types
    ├── specialized_node_invite.rs  # Invite protocol types
    └── autonat_v2/           # AutoNAT v2 behaviour
```

## Key Types

### NetworkManager (Actor)

```rust
// src/lib.rs
pub struct NetworkManager {
    swarm: Box<Swarm<Behaviour>>,              // libp2p swarm
    event_dispatcher: Arc<dyn NetworkEventDispatcher>, // Event delivery
    discovery: Discovery,                       // Peer discovery state
    pending_dial: HashMap<PeerId, oneshot::Sender<...>>,
    pending_bootstrap: HashMap<QueryId, oneshot::Sender<...>>,
    pending_blob_queries: HashMap<QueryId, oneshot::Sender<...>>,
    metrics: Metrics,
}

impl Actor for NetworkManager { ... }
impl Handler<NetworkMessage> for NetworkManager { ... }
impl StreamHandler<FromSwarm> for NetworkManager { ... }
```

### NetworkClient (Async API)

```rust
// primitives/src/client.rs
pub struct NetworkClient {
    network_manager: LazyRecipient<NetworkMessage>,
}

impl NetworkClient {
    // Core operations
    pub async fn subscribe(&self, topic: IdentTopic) -> eyre::Result<IdentTopic>;
    pub async fn unsubscribe(&self, topic: IdentTopic) -> eyre::Result<IdentTopic>;
    pub async fn publish(&self, topic: TopicHash, data: Vec<u8>) -> eyre::Result<MessageId>;
    pub async fn open_stream(&self, peer_id: PeerId) -> eyre::Result<Stream>;
    
    // Connection management
    pub async fn dial(&self, peer_addr: Multiaddr) -> eyre::Result<()>;
    pub async fn listen_on(&self, addr: Multiaddr) -> eyre::Result<()>;
    pub async fn bootstrap(&self) -> eyre::Result<()>;
    
    // Peer info
    pub async fn peer_count(&self) -> usize;
    pub async fn mesh_peer_count(&self, topic: TopicHash) -> usize;
    pub async fn mesh_peers(&self, topic: TopicHash) -> Vec<PeerId>;
    
    // Blob discovery
    pub async fn announce_blob(&self, blob_id, context_id, size) -> eyre::Result<()>;
    pub async fn query_blob(&self, blob_id, context_id) -> eyre::Result<Vec<PeerId>>;
    pub async fn request_blob(&self, blob_id, context_id, peer_id, auth) -> eyre::Result<Option<Vec<u8>>>;
}
```

### NetworkEvent (Outgoing Events)

```rust
// primitives/src/messages.rs
pub enum NetworkEvent {
    ListeningOn { listener_id, address },
    Subscribed { peer_id, topic },
    Unsubscribed { peer_id, topic },
    Message { id: MessageId, message: Message },  // ← Gossipsub message
    StreamOpened { peer_id, stream: Box<Stream>, protocol },
    BlobRequested { blob_id, context_id, requesting_peer },
    BlobProvidersFound { blob_id, context_id, providers },
    BlobDownloaded { blob_id, context_id, data, from_peer },
    BlobDownloadFailed { blob_id, context_id, from_peer, error },
    SpecializedNodeVerificationRequest { ... },
    SpecializedNodeInvitationResponse { ... },
}
```

### NetworkEventDispatcher (Event Delivery)

```rust
// primitives/src/messages.rs
/// Trait for dispatching network events.
/// Implemented by calimero-node to receive events from NetworkManager.
pub trait NetworkEventDispatcher: Send + Sync {
    /// Dispatch a network event. Returns true if delivered, false if dropped.
    fn dispatch(&self, event: NetworkEvent) -> bool;
}
```

## Key Concepts

### Gossipsub (Pub/Sub)

- **One topic per context**: Topic ID = `ContextId.to_string()`
- **All context members subscribe** to receive state deltas
- **Message authentication**: All messages signed with node identity

```rust
// NOTE: Error handling simplified - see NetworkClient for full API

// Subscribe to context
network_client.subscribe(IdentTopic::new(context_id.to_string())).await?;

// Publish delta
network_client.publish(topic.hash(), delta_bytes).await?;
```

### Direct Streams

- **Point-to-point** bidirectional communication
- **Used for**: Sync requests, blob transfers, large payloads
- **Protocol**: `/calimero/stream/0.0.2`

```rust
// NOTE: Error handling simplified - see NetworkClient::open_stream for full pattern

// Open stream to peer
let stream = network_client.open_stream(peer_id).await?;

// Send/receive messages
stream.send(Message::new(data)).await?;
let response = stream.next().await?;
```

### Peer Discovery

| Mechanism | Scope | When Used |
|-----------|-------|-----------|
| **mDNS** | Local network | Always (if enabled) |
| **Kademlia DHT** | Internet | Peer routing, blob discovery |
| **Rendezvous** | Internet | Bootstrap, registration |
| **Bootstrap nodes** | Internet | Initial network entry |

### NAT Traversal

| Protocol | Purpose |
|----------|---------|
| **AutoNAT** | Detect NAT type and reachability |
| **Relay** | Route traffic through relay nodes |
| **DCUtR** | Hole punching for direct connections |

## Patterns

### Adding a New Command Handler

```rust
// NOTE: Simplified for illustration - see existing handlers for full pattern

// 1. Add message type to primitives/src/messages.rs
#[derive(Clone, Debug)]
pub struct MyCommand { pub data: String }

impl actix::Message for MyCommand {
    type Result = eyre::Result<String>;
}

// 2. Add variant to NetworkMessage
pub enum NetworkMessage {
    // ...
    MyCommand {
        request: MyCommand,
        outcome: oneshot::Sender<<MyCommand as actix::Message>::Result>,
    },
}

// 3. Create handler in src/handlers/commands/my_command.rs
//    This implements the actual command logic
impl Handler<MyCommand> for NetworkManager {
    type Result = <MyCommand as Message>::Result;

    fn handle(&mut self, cmd: MyCommand, _ctx: &mut Context<Self>) -> Self::Result {
        // Implementation
        Ok(format!("processed: {}", cmd.data))
    }
}

// 4. Add dispatch in src/handlers/commands.rs
//    This routes the NetworkMessage variant to the Handler impl above
NetworkMessage::MyCommand { request, outcome } => {
    // forward_handler: invokes Handler::handle() and sends result via oneshot
    self.forward_handler(ctx, request, outcome);
}

// 5. Add client method in primitives/src/client.rs
pub async fn my_command(&self, data: String) -> eyre::Result<String> {
    let (tx, rx) = oneshot::channel();
    self.network_manager
        .send(NetworkMessage::MyCommand {
            request: MyCommand { data },
            outcome: tx,
        })
        .await
        .expect("Mailbox not dropped");
    rx.await.expect("Mailbox not dropped")
}
```

### Adding a New Swarm Event Handler

```rust
// NOTE: Simplified for illustration - see existing handlers for full pattern

// 1. Create handler file: src/handlers/stream/swarm/my_protocol.rs
use super::{EventHandler, NetworkManager};
use calimero_network_primitives::messages::NetworkEvent;

impl EventHandler<MyProtocolEvent> for NetworkManager {
    fn handle(&mut self, event: MyProtocolEvent) {
        match event {
            MyProtocolEvent::SomethingHappened { data } => {
                let _ignored = self.event_dispatcher.dispatch(
                    NetworkEvent::MyEvent { data }
                );
            }
        }
    }
}

// 2. Add module in src/handlers/stream/swarm.rs
mod my_protocol;

// 3. Add dispatch in FromSwarm handler
BehaviourEvent::MyProtocol(event) => EventHandler::handle(self, event),
```

## Key Files

| File | Purpose |
|------|---------|
| `src/lib.rs` | NetworkManager actor definition |
| `src/behaviour.rs` | Composed Behaviour with all protocols |
| `src/handlers/commands.rs` | Command dispatch (NetworkMessage) |
| `src/handlers/stream/swarm.rs` | Event dispatch (SwarmEvent) |
| `src/handlers/stream/swarm/gossipsub.rs` | Gossip message → NetworkEvent |
| `src/discovery.rs` | Discovery coordination logic |
| `src/discovery/state.rs` | Peer tracking, reachability state |
| `primitives/src/config.rs` | NetworkConfig and sub-configs |
| `primitives/src/messages.rs` | NetworkMessage, NetworkEvent |
| `primitives/src/client.rs` | NetworkClient async API |
| `primitives/src/stream.rs` | Stream type, protocol constants |

## JIT Index

```bash
# Find all command handlers
rg -n "impl Handler<" src/handlers/commands/

# Find all event handlers
rg -n "impl EventHandler<" src/handlers/stream/swarm/

# Find protocol definitions
rg -n "StreamProtocol::new" .

# Find gossipsub usage
rg -n "gossipsub\." src/

# Find network event dispatches
rg -n "event_dispatcher.dispatch" src/

# Find behaviour composition
rg -n "pub struct Behaviour" src/behaviour.rs

# Find discovery state mutations
rg -n "discovery.state\." src/
```

## Configuration

```rust
// primitives/src/config.rs
pub struct NetworkConfig {
    pub identity: Keypair,              // Node identity
    pub swarm: SwarmConfig,             // Listen addresses
    pub bootstrap: BootstrapConfig,     // Bootstrap nodes
    pub discovery: DiscoveryConfig,     // mDNS, rendezvous, relay, autonat
}

pub struct DiscoveryConfig {
    pub mdns: bool,                     // Enable mDNS (default: true)
    pub advertise_address: bool,        // Advertise public IP
    pub rendezvous: RendezvousConfig,   // Rendezvous settings
    pub relay: RelayConfig,             // Relay settings
    pub autonat: AutonatConfig,         // AutoNAT settings
}
```

## Debugging

```bash
# Enable network debug logging
RUST_LOG=calimero_network=debug,libp2p=debug merod --node node1 run

# Specific protocol debugging
RUST_LOG=libp2p_gossipsub=debug merod --node node1 run
RUST_LOG=libp2p_kad=debug merod --node node1 run
RUST_LOG=libp2p_swarm=debug merod --node node1 run

# Check peer connectivity
meroctl --node node1 peers ls

# Get peer details
meroctl --node node1 peers get <peer_id>

# Check listening addresses
lsof -i :<port>
```

## Common Gotchas

- **Port conflicts**: Check `lsof -i :<port>` before starting
- **Firewall**: May block P2P connections (especially UDP for QUIC)
- **mDNS scope**: Only works on local network (same subnet)
- **Bootstrap required**: Need bootstrap nodes for internet connectivity
- **PeerId derivation**: PeerId is derived from node's cryptographic identity
- **Topic format**: Gossipsub topic = `context_id.to_string()` (hex)
- **Message size**: Max 8MB per stream message (see `MAX_MESSAGE_SIZE`)

## Testing

### Unit Tests

```bash
cargo test -p calimero-network
```

### Integration with Simulation

The `calimero-node` crate includes a simulation framework (`tests/sync_sim/`) that models network behavior for protocol testing. See `crates/node/tests/sync_sim/AGENT_GUIDE.md` for details.

**What sim models**: Message delivery, latency, loss, reorder, partitions
**What sim doesn't model**: Discovery, NAT traversal, connection setup

## Related Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md) - Internal architecture and design decisions
- [PROTOCOLS.md](PROTOCOLS.md) - Wire protocol specifications
- [README.md](README.md) - Comprehensive networking guide with diagrams
- [crates/node/tests/sync_sim/AGENT_GUIDE.md](../node/tests/sync_sim/AGENT_GUIDE.md) - Simulation framework
