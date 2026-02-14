# Calimero Network Protocols

This document describes the wire protocols, message formats, and communication patterns used by the Calimero network layer.

## Table of Contents

- [Overview](#overview)
- [Transport Layer](#transport-layer)
- [Stream Protocols](#stream-protocols)
- [Gossipsub Topics](#gossipsub-topics)
- [Discovery Protocols](#discovery-protocols)
- [Message Flows](#message-flows)

## Overview

Calimero uses [libp2p](https://libp2p.io/) as its networking foundation, with custom protocols layered on top for application-specific communication.

```text
┌─────────────────────────────────────────────────────────────┐
│                    Application Layer                         │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────┐ │
│  │ State Sync      │  │ Blob Transfer   │  │ Node Invite │ │
│  │ Protocol        │  │ Protocol        │  │ Protocol    │ │
│  └────────┬────────┘  └────────┬────────┘  └──────┬──────┘ │
└───────────┼─────────────────────┼─────────────────┼────────┘
            │                     │                 │
┌───────────┼─────────────────────┼─────────────────┼────────┐
│           ▼                     ▼                 ▼        │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              libp2p Protocol Layer                   │   │
│  │  ┌──────────┐ ┌──────────┐ ┌────────────────────┐   │   │
│  │  │ Stream   │ │ Gossipsub│ │ Request-Response   │   │   │
│  │  │ Protocol │ │ (Topics) │ │ Protocol           │   │   │
│  │  └──────────┘ └──────────┘ └────────────────────┘   │   │
│  └─────────────────────────────────────────────────────┘   │
│                     libp2p Core                             │
└─────────────────────────────────────────────────────────────┘
```

## Transport Layer

### Supported Transports

| Transport | Encryption | Multiplexing | Use Case |
|-----------|------------|--------------|----------|
| **TCP** | TLS / Noise | Yamux | General connectivity |
| **QUIC** | Built-in TLS | Native | Improved performance, NAT traversal |

### Connection Security

All connections use authenticated encryption:

1. **TLS 1.3** (primary) - Standard transport security
2. **Noise Protocol** (fallback) - For environments where TLS is problematic

### Multiplexing

Yamux (Yet another Multiplexer) enables multiple logical streams over a single connection:

- Reduces connection overhead
- Enables concurrent request/response patterns
- Supports flow control per-stream

## Stream Protocols

### CALIMERO_STREAM_PROTOCOL

```
Protocol ID: /calimero/stream/0.0.2
```

**Purpose**: General-purpose bidirectional streams for sync operations.

**Use Cases**:
- State synchronization requests
- DAG delta transfers
- Tree node requests/responses
- Hash comparison sync

**Message Format**:

```text
┌────────────────────────────────────────────────┐
│           Length-Delimited Frame               │
├────────────────────────────────────────────────┤
│  4 bytes  │         Variable length            │
│  (length) │         (payload)                  │
├───────────┼────────────────────────────────────┤
│  u32 BE   │  Message { data: Vec<u8> }         │
└───────────┴────────────────────────────────────┘

Maximum frame size: 8 MB (8 * 1024 * 1024 bytes)
```

**Codec Implementation**: `MessageCodec` in `primitives/src/stream/codec.rs`

```rust
// Message structure
pub struct Message<'a> {
    pub data: Cow<'a, [u8]>,
}

// Usage
let stream = network.open_stream(peer_id).await?;
stream.send(Message::new(data)).await?;
let response = stream.recv().await?;
```

### CALIMERO_BLOB_PROTOCOL

```
Protocol ID: /calimero/blob/0.0.2
```

**Purpose**: Large binary object transfers between peers.

**Use Cases**:
- Application WASM binary distribution
- Large state snapshots
- Media/file transfers within contexts

**Message Format**: Same as `CALIMERO_STREAM_PROTOCOL`

**Flow**:

```text
Requester                              Provider
    │                                      │
    │──── BlobRequest(blob_id, ctx) ──────►│
    │                                      │
    │◄──── BlobResponse(data) ─────────────│
    │      or BlobNotFound                 │
    │                                      │
```

### CALIMERO_KAD_PROTO_NAME

```
Protocol ID: /calimero/kad/1.0.0
```

**Purpose**: Kademlia DHT for peer routing and content discovery.

**Distinct from IPFS**: Uses a separate DHT network with Calimero-specific bootstrap nodes.

**Use Cases**:
- Peer discovery
- Blob provider discovery (which peers have a specific blob)
- Distributed peer routing

## Gossipsub Topics

Calimero uses [Gossipsub](https://github.com/libp2p/specs/blob/master/pubsub/gossipsub/gossipsub-v1.0.md) for pub/sub messaging.

### Topic Structure

```
Topic ID: <context_id_hex>
Example:  "a1b2c3d4e5f6..."  (64 hex chars for 32-byte ContextId)
```

**One topic per context**: Each context (application instance) has exactly one gossip topic.

### Message Format

```text
┌────────────────────────────────────────────────┐
│              Gossipsub Message                  │
├────────────────────────────────────────────────┤
│ from: PeerId          (message author)         │
│ topic: TopicHash      (context ID)             │
│ data: Vec<u8>         (serialized payload)     │
│ sequence_number: u64  (ordering)               │
│ signature: Vec<u8>    (author signature)       │
└────────────────────────────────────────────────┘
```

### Message Authentication

All gossipsub messages are **signed** using the sender's node identity key:

```rust
gossipsub::MessageAuthenticity::Signed(keypair)
```

### Typical Payloads

| Payload Type | Description |
|--------------|-------------|
| `StateDelta` | CRDT state change broadcast |
| `DagEntry` | New DAG node announcement |
| `SyncRequest` | Request to initiate sync |

## Discovery Protocols

### Rendezvous Protocol

```
Namespace: /calimero/devnet/global
```

**Purpose**: Peer discovery through known rendezvous points.

**Flow**:

```text
New Node                    Rendezvous Server              Other Nodes
    │                              │                            │
    │── Register(namespace) ──────►│                            │
    │                              │                            │
    │◄─ RegisterOk ────────────────│                            │
    │                              │                            │
    │── Discover(namespace) ──────►│                            │
    │                              │                            │
    │◄─ Registrations [peers...] ──│                            │
    │                              │                            │
    │──────────────────── Dial ────┼───────────────────────────►│
    │                              │                            │
```

### mDNS (Multicast DNS)

**Purpose**: Local network peer discovery (LAN only).

**Enabled by**: `config.discovery.mdns = true`

**Behavior**:
- Broadcasts presence on local network
- Automatically discovers other Calimero nodes
- Zero configuration required

### Identify Protocol

```
Protocol ID: /calimero-network/<version>
```

**Purpose**: Exchange peer metadata on connection.

**Exchanged Information**:
- Supported protocols
- Listen addresses
- Agent version
- Public key

## Message Flows

### State Delta Broadcast

```text
Node A (writer)                Network                    Node B, C, D (subscribers)
      │                           │                              │
      │  1. Write to CRDT         │                              │
      │  2. Generate delta        │                              │
      │                           │                              │
      │── Gossipsub.Publish ─────►│                              │
      │   (topic=context_id,      │                              │
      │    data=serialized_delta) │                              │
      │                           │                              │
      │                           │── Gossipsub.Message ────────►│
      │                           │                              │
      │                           │                    3. Deserialize delta
      │                           │                    4. Apply to local CRDT
      │                           │                    5. Update DAG
      │                           │                              │
```

### Sync Stream Flow

```text
Initiator                                              Responder
    │                                                       │
    │  1. Detect missing state (DAG heads differ)           │
    │                                                       │
    │── OpenStream(CALIMERO_STREAM_PROTOCOL) ──────────────►│
    │                                                       │
    │◄─ StreamOpened ───────────────────────────────────────│
    │                                                       │
    │── InitMessage(context_id, party_id) ─────────────────►│
    │                                                       │
    │◄─ DagHeadsResponse(heads, root_hash) ─────────────────│
    │                                                       │
    │  2. Compare root hashes                               │
    │                                                       │
    │── TreeNodeRequest(path) ─────────────────────────────►│
    │                                                       │
    │◄─ TreeNodeResponse(node, children) ───────────────────│
    │                                                       │
    │  ... continue tree traversal ...                      │
    │                                                       │
    │── SyncComplete ──────────────────────────────────────►│
    │                                                       │
    │◄─ Close ──────────────────────────────────────────────│
    │                                                       │
```

### Blob Discovery and Transfer

```text
Requester                      DHT                       Provider
    │                           │                            │
    │  1. Need blob_id for context                           │
    │                                                        │
    │── Kad.GetProviders(key) ─►│                            │
    │                           │                            │
    │◄─ Providers [peer_ids] ───│                            │
    │                           │                            │
    │  2. Choose provider                                    │
    │                                                        │
    │── OpenStream(CALIMERO_BLOB_PROTOCOL) ─────────────────►│
    │                                                        │
    │── BlobRequest(blob_id, context_id) ──────────────────►│
    │                                                        │
    │◄─ BlobData(bytes) ─────────────────────────────────────│
    │                                                        │
    │  3. Verify hash matches blob_id                        │
    │  4. Store locally                                      │
    │  5. Announce as provider                               │
    │                                                        │
    │── Kad.StartProviding(key) ►│                           │
    │                            │                           │
```

## Protocol Constants

| Constant | Value | Location |
|----------|-------|----------|
| `MAX_MESSAGE_SIZE` | 8 MB | `primitives/src/stream.rs` |
| `CALIMERO_STREAM_PROTOCOL` | `/calimero/stream/0.0.2` | `primitives/src/stream.rs` |
| `CALIMERO_BLOB_PROTOCOL` | `/calimero/blob/0.0.2` | `primitives/src/stream.rs` |
| `CALIMERO_KAD_PROTO_NAME` | `/calimero/kad/1.0.0` | `src/behaviour.rs` |
| `DEFAULT_PORT` | 2428 | `primitives/src/config.rs` |

## Security Considerations

1. **Message Authentication**: All gossipsub messages are cryptographically signed
2. **Transport Encryption**: All connections use TLS or Noise
3. **Peer Identity**: PeerIds are derived from cryptographic keys
4. **Context Isolation**: Different contexts use different gossip topics

## Debugging

### Enable Protocol Logging

```bash
RUST_LOG=calimero_network=debug,libp2p=debug merod --node node1 run
```

### Useful Debug Filters

```bash
# Gossipsub only
RUST_LOG=libp2p_gossipsub=debug

# Stream protocol
RUST_LOG=libp2p_stream=debug

# Kademlia DHT
RUST_LOG=libp2p_kad=debug

# All connection events
RUST_LOG=libp2p_swarm=debug
```

## References

- [libp2p Specification](https://github.com/libp2p/specs)
- [Gossipsub v1.1](https://github.com/libp2p/specs/blob/master/pubsub/gossipsub/gossipsub-v1.1.md)
- [Kademlia Paper](https://pdos.csail.mit.edu/~petar/papers/maymounkov-kademlia-lncs.pdf)
- [Noise Protocol Framework](https://noiseprotocol.org/)
