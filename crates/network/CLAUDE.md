# calimero-network - P2P Networking

Peer-to-peer networking layer using libp2p for peer discovery, gossipsub messaging, and direct streams.

- **Crate**: `calimero-network`
- **Entry**: `src/lib.rs`
- **Frameworks**: libp2p (P2P), tokio (async)

## Build & Test

```bash
cargo build -p calimero-network
cargo test -p calimero-network
```

## File Layout

```
src/
├── lib.rs                          # NetworkConfig, public API
├── behaviour.rs                    # Composed libp2p behaviour
├── discovery/
│   └── state.rs                    # Discovery state
├── handlers/
│   ├── commands/
│   │   ├── subscribe.rs            # Topic subscription
│   │   ├── unsubscribe.rs
│   │   ├── publish.rs              # Gossipsub publish
│   │   ├── open_stream.rs
│   │   ├── dial.rs
│   │   ├── announce_blob.rs
│   │   ├── query_blob.rs
│   │   └── request_blob.rs
│   └── stream/
│       ├── incoming.rs
│       ├── rendezvous.rs
│       └── swarm/
│           ├── gossipsub.rs        # Incoming gossip messages
│           ├── mdns.rs             # Local peer discovery
│           ├── kad.rs              # Kademlia DHT
│           ├── ping.rs
│           ├── identify.rs
│           ├── autonat.rs
│           ├── dcutr.rs
│           ├── relay.rs
│           └── rendezvous.rs
primitives/src/lib.rs               # PeerId, shared types
```

## Key Concepts

### Gossipsub

- Each context = one gossip topic (keyed by `ContextId`)
- All context members subscribe; deltas broadcast to all
- Handler: `src/handlers/stream/swarm/gossipsub.rs`

### Direct Streams

Point-to-point for sync requests, blob transfers, direct messaging.

### Peer Discovery

- **mDNS**: local network only
- **Bootstrap nodes**: internet connectivity
- **Kademlia DHT**: peer routing

## Patterns

### Subscribe & Publish

```rust
network.subscribe(context_id).await?;
network.publish(context_id, delta_bytes).await?;
```

### Open Direct Stream

```rust
let stream = network.open_stream(peer_id, protocol).await?;
stream.send(message).await?;
let response = stream.receive().await?;
```

### NetworkConfig

```rust
pub struct NetworkConfig {
    pub swarm_port:       u16,
    pub bootstrap_nodes:  Vec<Multiaddr>,
    pub mdns_enabled:     bool,
}
```

## Key Files

| File | Purpose |
|---|---|
| `src/lib.rs` | Network init, `NetworkConfig` |
| `src/behaviour.rs` | Composed libp2p behaviour |
| `src/handlers/stream/swarm/gossipsub.rs` | Incoming gossip |
| `src/handlers/commands/subscribe.rs` | Topic subscription |
| `src/handlers/commands/publish.rs` | Message publishing |

## Quick Search

```bash
rg -n "GossipsubEvent" src/
rg -n "Stream" src/handlers/
rg -n "discover" src/discovery/
rg -n "protocol" src/
```

## Debugging

```bash
RUST_LOG=calimero_network=debug,libp2p=debug merod --node node1 run
meroctl --node node1 peers ls
meroctl --node node1 peers get <peer_id>
```

## Gotchas

- Ports must be free — check with `lsof -i :<port>`
- mDNS only works on the local network segment
- Bootstrap nodes required for internet-facing deployments
- `PeerId` is derived from the node's identity key
- Firewall rules may silently block incoming connections
