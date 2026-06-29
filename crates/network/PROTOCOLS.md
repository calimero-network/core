# Calimero Network Protocols

This document describes the wire protocols, message formats, and communication patterns used by the Calimero network layer.

> **Scope note.** This reference covers the core transport, stream, gossipsub, and discovery protocols. It predates several later networking features — gossipsub peer scoring, mesh tuning, `flood_publish`, the persistent peer-address cache, ping-failure connection reaping, AutoNAT v2, DCUtR hole-punching, and the specialized-node-invite request-response protocol. For those, see the docs site: [Networking & the wire protocol](https://calimero.network/protocol/networking/) and [Networking & Discovery](https://calimero.network/operate/networking/).

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
*(defined in `primitives/src/stream.rs` as `CALIMERO_STREAM_PROTOCOL`)*

**Purpose**: General-purpose bidirectional streams for sync operations.

**Use Cases**:
- State synchronization requests
- DAG delta transfers
- Tree node requests/responses
- Hash comparison sync

**Message Format** *(source: `primitives/src/stream/codec.rs:MessageCodec`)*:

```text
┌────────────────────────────────────────────────┐
│           Length-Delimited Frame               │
├────────────────────────────────────────────────┤
│  4 bytes  │         Variable length            │
│  (length) │         (payload)                  │
├───────────┼────────────────────────────────────┤
│  u32 BE   │  Message { data: Vec<u8> }         │
└───────────┴────────────────────────────────────┘

Maximum frame size: 8 MB (defined as MAX_MESSAGE_SIZE in primitives/src/stream.rs)
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

**Message Format**: Length-delimited frames (same codec as `CALIMERO_STREAM_PROTOCOL`). The first frame each way is a JSON `BlobRequest` / `BlobResponse` header; the payload that follows is a **stream of Borsh `BlobChunk` frames**, not a single response.

**Flow** (chunked stream):

```text
Requester                              Provider
    │                                      │
    │── BlobRequest { blob_id, ctx, auth? }►│
    │                                      │
    │◄── BlobResponse { found, size? } ─────│
    │                                      │
    │◄── BlobChunk { data } ────────────────│  (one per stored chunk)
    │◄── BlobChunk { data } ────────────────│
    │              ...                      │
    │◄── BlobChunk { data: [] } ────────────│  (empty chunk = end of stream)
    │                                      │
```

If the provider does not hold the blob it replies `BlobResponse { found: false }` and sends no chunks. The requester bounds the transfer with a 60s overall and 30s per-chunk timeout, and recomputes the `BlobId` from the assembled bytes before accepting them. Non-public blobs require a signed `BlobAuth` (member of `context_id`) on the request.

### CALIMERO_KAD_PROTO_NAME

```
Protocol ID: /calimero/kad/1.0.0
```

**Purpose**: Kademlia DHT for peer routing and content discovery.

**Distinct from IPFS**: Uses a separate DHT network with Calimero-specific bootstrap nodes.

**Use Cases**:
- Peer discovery
- Blob discovery via **custom Kademlia records** (see below) — *not* libp2p provider records
- Distributed peer routing

**Blob discovery uses ordinary Kad records, not `StartProviding` / `GetProviders`.** To announce a blob, a node `put_record`s a record keyed by `context_id ‖ blob_id` whose value is `local_peer_id ‖ size` (size as little-endian `u64`), with `Quorum::One`. To discover, a node `get_record`s the same `context_id ‖ blob_id` key and dials the advertised peer. Keys are always context-scoped — global (context-less) blob queries are not supported.

## Gossipsub Topics

Calimero uses [Gossipsub](https://github.com/libp2p/specs/blob/master/pubsub/gossipsub/gossipsub-v1.0.md) for pub/sub messaging.

### Topic Structure

A node subscribes to a topic per overlay it belongs to, not just one per context:

```
Topic ID: <context_id>       a context's data operations and heartbeats
          ns/<hex>           a namespace's root governance operations
          group/<hex>        a group's governance operations
```

So a single node typically holds several topics at once — one per context, namespace, and group it follows.

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
Configured namespace: /calimero/devnet/global   (bootstrap / namespace-join path)
Per-overlay namespaces (derived from the subscribed gossipsub topic):
    ns/<hex>      → /calimero/ns/<hex>
    group/<hex>   → /calimero/grp/<hex>
    <context-id>  → /calimero/ctx/<id>
```

**Purpose**: Peer discovery through known rendezvous points.

**Per-overlay, not one global namespace.** Beyond the configured global namespace, a node registers and discovers under one key *per overlay it follows*, derived deterministically from the gossipsub topic string. `discover` on such a key returns only co-members of that exact namespace, group, or context — relevant peers by construction. The global namespace is used for the bootstrap / namespace-join path (finding the members of a namespace the node does not belong to yet).

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
    │── Kad.get_record(ctx‖blob_id) ►│                       │
    │                           │                            │
    │◄─ Record(peer_id‖size) ───│                            │
    │                           │                            │
    │  2. Dial the advertised peer                           │
    │                                                        │
    │── OpenStream(CALIMERO_BLOB_PROTOCOL) ─────────────────►│
    │── BlobRequest { blob_id, context_id, auth? } ─────────►│
    │                                                        │
    │◄─ BlobResponse { found, size? } ───────────────────────│
    │◄─ BlobChunk { data } ... BlobChunk { data: [] } ───────│
    │                                                        │
    │  3. Verify recomputed id matches blob_id               │
    │  4. Store locally                                      │
    │  5. Announce own record                                │
    │                                                        │
    │── Kad.put_record(ctx‖blob_id, peer_id‖size) ►│         │
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
