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
├── discovery/
│   ├── mod.rs                # Discovery module
│   ├── mdns.rs               # mDNS discovery
│   └── state.rs              # Discovery state
├── handlers/
│   ├── mod.rs                # Handler module
│   ├── gossipsub.rs          # Gossipsub message handling
│   ├── stream.rs             # Stream handling
│   └── swarm.rs              # Swarm events
├── stream/
│   ├── mod.rs                # Stream module
│   └── codec.rs              # Stream codec
├── config.rs                 # Network configuration
├── types.rs                  # Network types
└── events.rs                 # Network events
primitives/                   # calimero-network-primitives
├── src/
│   ├── lib.rs                # Shared types
│   └── ...
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

- ✅ DO: Follow pattern in `src/handlers/gossipsub.rs`

```rust
// src/handlers/gossipsub.rs
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

| File                        | Purpose                    |
| --------------------------- | -------------------------- |
| `src/lib.rs`                | Network initialization     |
| `src/handlers/gossipsub.rs` | Gossip message handling    |
| `src/handlers/stream.rs`    | Stream event handling      |
| `src/discovery/mdns.rs`     | Local peer discovery       |
| `src/stream/codec.rs`       | Message encoding           |
| `primitives/src/lib.rs`     | PeerId, NetworkEvent types |

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
// NetworkConfig in src/config.rs
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
