# calimero-network - P2P Networking

Peer-to-peer networking layer using libp2p for peer discovery, gossipsub messaging, and direct streams.

## Package Identity

- **Crate**: `calimero-network`
- **Entry**: `src/lib.rs`
- **Framework**: libp2p (P2P), tokio (async)

## Commands

```bash
# Build
cargo build -p calimero-network

# Test
cargo test -p calimero-network
```

## File Organization

```
src/
├── lib.rs                    # Public exports, NetworkConfig
├── behaviour.rs              # Network behaviour definition
├── discovery.rs              # Discovery module parent
├── discovery/
│   ├── state.rs              # Discovery state
│   └── state_tests.rs        # Discovery state tests
├── handlers.rs               # Handlers module parent
├── handlers/
│   ├── commands.rs           # Command handlers parent
│   ├── commands/
│   │   ├── subscribe.rs      # Topic subscription
│   │   ├── unsubscribe.rs    # Topic unsubscription
│   │   ├── publish.rs        # Message publishing
│   │   ├── open_stream.rs    # Stream opening
│   │   ├── dial.rs           # Dial peer
│   │   ├── listen.rs         # Listen for connections
│   │   ├── bootstrap.rs      # Bootstrap network
│   │   ├── peer_count.rs     # Get peer count
│   │   ├── mesh_peer_count.rs # Get mesh peer count
│   │   ├── mesh_peers.rs     # Get mesh peers
│   │   ├── announce_blob.rs  # Announce blob
│   │   ├── query_blob.rs     # Query blob
│   │   ├── request_blob.rs   # Request blob
│   │   ├── send_specialized_node_invitation_response.rs  # Send invitation response
│   │   └── send_specialized_node_verification_request.rs # Send verification request
│   ├── stream.rs             # Stream handlers parent
│   └── stream/
│       ├── incoming.rs       # Incoming stream handling
│       ├── rendezvous.rs     # Rendezvous protocol
│       ├── swarm.rs          # Swarm event handlers parent
│       └── swarm/
│           ├── gossipsub.rs  # Gossipsub message handling
│           ├── mdns.rs       # mDNS discovery
│           ├── kad.rs        # Kademlia DHT
│           ├── ping.rs       # Ping protocol
│           ├── identify.rs   # Identify protocol
│           ├── autonat.rs    # AutoNAT protocol
│           ├── dcutr.rs      # DCUtR protocol
│           ├── relay.rs      # Relay protocol
│           ├── rendezvous.rs # Rendezvous protocol
│           └── specialized_node_invite.rs  # Specialized node invite protocol
primitives/                   # calimero-network-primitives
└── src/
    └── lib.rs                # Shared types (PeerId, etc.)
```

## Key Concepts

### Gossipsub

Pub/sub messaging for broadcasting state deltas:

- Each context = one gossip topic
- All context members subscribe to topic
- Deltas propagate to all subscribers

### Direct Streams

Point-to-point connections for:

- Sync requests
- Blob transfers
- Direct messaging

### Peer Discovery

- mDNS for local network discovery
- Bootstrap nodes for internet discovery
- Kademlia DHT for peer routing

## Patterns

### Network Event Handling

- ✅ DO: Follow pattern in `src/handlers/stream/swarm/gossipsub.rs`

```rust
// src/handlers/stream/swarm/gossipsub.rs
pub fn handle_gossipsub_event(
    event: GossipsubEvent,
    state: &mut NetworkState,
) -> Option<NetworkEvent> {
    match event {
        GossipsubEvent::Message { message, .. } => {
            // Handle incoming message
        }
        // ...
    }
}
```

### Opening a Stream

```rust
// Pattern for opening direct streams
let stream = network.open_stream(peer_id, protocol).await?;
stream.send(message).await?;
let response = stream.receive().await?;
```

### Subscribing to Topic

```rust
// Subscribe to context topic
network.subscribe(context_id).await?;

// Publish to topic
network.publish(context_id, delta).await?;
```

## Key Files

| File                                     | Purpose                 |
| ---------------------------------------- | ----------------------- |
| `src/lib.rs`                             | Network initialization  |
| `src/behaviour.rs`                       | Network behaviour       |
| `src/handlers/stream/swarm/gossipsub.rs` | Gossip message handling |
| `src/handlers/stream/swarm/mdns.rs`      | Local peer discovery    |
| `src/handlers/commands/subscribe.rs`     | Topic subscription      |
| `src/handlers/commands/publish.rs`       | Message publishing      |
| `primitives/src/lib.rs`                  | Shared types            |

## JIT Index

```bash
# Find network protocols
rg -n "protocol" src/

# Find gossipsub handlers
rg -n "GossipsubEvent" src/

# Find stream handling
rg -n "Stream" src/handlers/

# Find peer discovery
rg -n "discover" src/discovery/
```

## Configuration

```rust
// NetworkConfig in src/lib.rs
pub struct NetworkConfig {
    pub swarm_port: u16,          // libp2p swarm port
    pub bootstrap_nodes: Vec<Multiaddr>,
    pub mdns_enabled: bool,
}
```

## Debugging

```bash
# Enable network debug logging
RUST_LOG=calimero_network=debug,libp2p=debug merod --node node1 run

# Check peer connectivity
meroctl --node node1 peers ls

# Get peer details
meroctl --node node1 peers get <peer_id>
```

## Common Gotchas

- Ports must be available (check with `lsof -i :<port>`)
- Firewall may block P2P connections
- mDNS only works on local network
- Bootstrap nodes required for internet connectivity
- PeerIds are derived from node identity keys
