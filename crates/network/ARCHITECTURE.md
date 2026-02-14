# Network Architecture

This document describes the internal architecture, state machines, and design decisions of the `calimero-network` crate.

## Table of Contents

- [Component Overview](#component-overview)
- [NetworkManager Actor](#networkmanager-actor)
- [Behaviour Composition](#behaviour-composition)
- [Discovery State Machine](#discovery-state-machine)
- [Connection Lifecycle](#connection-lifecycle)
- [Message Flow Diagrams](#message-flow-diagrams)
- [Design Decisions](#design-decisions)

## Component Overview

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                             calimero-network                                 │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │                        NetworkManager (Actor)                          │ │
│  │                                                                        │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌──────────────┐  │ │
│  │  │   Swarm     │  │  Discovery  │  │   Event     │  │   Pending    │  │ │
│  │  │ <Behaviour> │  │   State     │  │ Dispatcher  │  │  Operations  │  │ │
│  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └──────┬───────┘  │ │
│  │         │                │                │                │          │ │
│  └─────────┼────────────────┼────────────────┼────────────────┼──────────┘ │
│            │                │                │                │            │
│            ▼                │                │                │            │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                      Behaviour (Composed)                            │   │
│  │  ┌─────────┐ ┌───────┐ ┌──────┐ ┌───────────┐ ┌───────┐ ┌────────┐  │   │
│  │  │Gossipsub│ │  Kad  │ │ mDNS │ │Rendezvous │ │ Relay │ │ DCUtR  │  │   │
│  │  └─────────┘ └───────┘ └──────┘ └───────────┘ └───────┘ └────────┘  │   │
│  │  ┌─────────┐ ┌───────┐ ┌──────┐ ┌───────────┐                       │   │
│  │  │ AutoNAT │ │Identify│ │ Ping │ │  Stream   │                       │   │
│  │  └─────────┘ └───────┘ └──────┘ └───────────┘                       │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## NetworkManager Actor

The `NetworkManager` is an Actix actor that orchestrates all network operations.

### State

```rust
pub struct NetworkManager {
    // Core networking
    swarm: Box<Swarm<Behaviour>>,           // libp2p swarm
    
    // Event delivery
    event_dispatcher: Arc<dyn NetworkEventDispatcher>,
    
    // Discovery coordination
    discovery: Discovery,
    
    // Pending async operations
    pending_dial: HashMap<PeerId, oneshot::Sender<Result<()>>>,
    pending_bootstrap: HashMap<QueryId, oneshot::Sender<Result<()>>>,
    pending_blob_queries: HashMap<QueryId, oneshot::Sender<Result<Vec<PeerId>>>>,
    
    // Observability
    metrics: Metrics,
}
```

### Message Handlers

```text
NetworkManager handles:
├── NetworkMessage (from NetworkClient)
│   ├── Dial          → swarm.dial()
│   ├── ListenOn      → swarm.listen_on()
│   ├── Bootstrap     → kad.bootstrap()
│   ├── Subscribe     → gossipsub.subscribe()
│   ├── Unsubscribe   → gossipsub.unsubscribe()
│   ├── Publish       → gossipsub.publish()
│   ├── OpenStream    → stream.new_control().open_stream()
│   ├── PeerCount     → swarm.connected_peers().count()
│   ├── MeshPeers     → gossipsub.mesh_peers()
│   ├── MeshPeerCount → gossipsub.mesh_peers().count()
│   ├── AnnounceBlob  → kad.start_providing()
│   ├── QueryBlob     → kad.get_providers()
│   └── RequestBlob   → open stream + send request
│
├── FromSwarm (SwarmEvent stream)
│   ├── Behaviour events (gossipsub, kad, mdns, etc.)
│   ├── ConnectionEstablished
│   ├── ConnectionClosed
│   ├── ListeningOn
│   ├── ExternalAddrConfirmed
│   └── ... other swarm events
│
├── FromIncoming (incoming stream connections)
│   └── StreamOpened → dispatch NetworkEvent::StreamOpened
│
└── RendezvousTick (periodic timer)
    └── Trigger rendezvous discovery
```

### Actor Lifecycle

```text
                    ┌─────────────┐
                    │   Created   │
                    └──────┬──────┘
                           │ Actor::started()
                           ▼
                    ┌─────────────┐
                    │   Started   │
                    └──────┬──────┘
                           │
          ┌────────────────┼────────────────┐
          │                │                │
          ▼                ▼                ▼
   ┌──────────────┐ ┌──────────────┐ ┌────────────────┐
   │ Accept       │ │ Accept       │ │ Start          │
   │ Stream       │ │ Blob         │ │ Rendezvous     │
   │ Protocol     │ │ Protocol     │ │ Timer          │
   └──────────────┘ └──────────────┘ └────────────────┘
          │                │                │
          └────────────────┼────────────────┘
                           │
                           ▼
                    ┌─────────────┐
                    │   Running   │◄────────────┐
                    └──────┬──────┘             │
                           │                    │
                           ▼                    │
              ┌────────────────────────┐        │
              │ Process events:        │        │
              │ • SwarmEvents          │────────┘
              │ • NetworkMessages      │
              │ • IncomingStreams      │
              │ • RendezvousTicks      │
              └────────────────────────┘
```

## Behaviour Composition

The `Behaviour` struct combines 11 libp2p behaviours into a single composite:

```rust
#[derive(NetworkBehaviour)]
pub struct Behaviour {
    // Discovery
    pub mdns: Toggle<mdns::Behaviour>,        // Local network discovery
    pub kad: kad::Behaviour<MemoryStore>,     // DHT for routing & blob discovery
    pub rendezvous: rendezvous::client::Behaviour, // Bootstrap discovery
    
    // Pub/Sub
    pub gossipsub: gossipsub::Behaviour,      // Topic-based messaging
    
    // Direct Communication
    pub stream: libp2p_stream::Behaviour,     // Point-to-point streams
    
    // NAT Traversal
    pub autonat: autonat::Behaviour,          // NAT detection
    pub relay: relay::client::Behaviour,      // Relay circuit
    pub dcutr: dcutr::Behaviour,              // Hole punching
    
    // Meta
    pub identify: identify::Behaviour,        // Protocol exchange
    pub ping: ping::Behaviour,                // Liveness check
    
    // Application Protocol
    pub specialized_node_invite: request_response::Behaviour<...>,
}
```

### Event Flow

```text
libp2p internal
      │
      ▼
SwarmEvent<BehaviourEvent>
      │
      ▼
┌─────────────────────────────────────────────────────────────┐
│                 FromSwarm StreamHandler                      │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  match BehaviourEvent {                                      │
│      Gossipsub(e)   → EventHandler::handle(self, e)         │
│      Kad(e)         → EventHandler::handle(self, e)         │
│      Mdns(e)        → EventHandler::handle(self, e)         │
│      Rendezvous(e)  → EventHandler::handle(self, e)         │
│      Relay(e)       → EventHandler::handle(self, e)         │
│      Dcutr(e)       → EventHandler::handle(self, e)         │
│      Autonat(e)     → EventHandler::handle(self, e)         │
│      Identify(e)    → EventHandler::handle(self, e)         │
│      Ping(e)        → EventHandler::handle(self, e)         │
│      Stream(())     → (no action)                           │
│      SpecializedNodeInvite(e) → EventHandler::handle(...)   │
│  }                                                           │
│                                                              │
│  match SwarmEvent {                                          │
│      ConnectionEstablished → update discovery state          │
│      ConnectionClosed      → cleanup discovery state         │
│      NewListenAddr         → dispatch ListeningOn            │
│      ExternalAddrConfirmed → broadcast rendezvous, update NAT│
│      ...                                                     │
│  }                                                           │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

## Discovery State Machine

The `DiscoveryState` tracks peer information and reachability status.

### Peer States

```text
                    ┌─────────────────┐
                    │    Unknown      │
                    └────────┬────────┘
                             │ Discovered (mDNS/Rendezvous/Kad)
                             ▼
                    ┌─────────────────┐
                    │   Discovered    │
                    └────────┬────────┘
                             │ Connection established
                             ▼
                    ┌─────────────────┐
            ┌───────│   Connected     │───────┐
            │       └────────┬────────┘       │
            │                │                │
   Identify │                │ Connection    │ Becomes
   Exchange │                │ Closed        │ Relay/Rendezvous
            │                │                │
            ▼                ▼                ▼
   ┌────────────────┐ ┌────────────┐ ┌────────────────────┐
   │ Protocols      │ │ Previously │ │ Special Role       │
   │ Known          │ │ Connected  │ │ (Relay/Rendezvous) │
   └────────────────┘ └────────────┘ └────────────────────┘
```

### Rendezvous Registration States

```text
    ┌───────────────┐
    │  Discovered   │ ← Initial state when rendezvous peer found
    └───────┬───────┘
            │ register() called
            ▼
    ┌───────────────┐
    │  Requested    │ ← Waiting for registration response
    └───────┬───────┘
            │
      ┌─────┴─────┐
      │           │
      ▼           ▼
┌──────────┐  ┌──────────┐
│Registered│  │  Failed  │
└────┬─────┘  └────┬─────┘
     │             │
     │ TTL expires │ Retry
     │             │
     ▼             │
┌──────────┐       │
│ Expired  │───────┘
└──────────┘
```

### Relay Reservation States

```text
    ┌───────────────┐
    │  Discovered   │ ← Initial state when relay peer found
    └───────┬───────┘
            │ listen_on(relay_addr) called
            ▼
    ┌───────────────┐
    │  Requested    │ ← Waiting for reservation
    └───────┬───────┘
            │
      ┌─────┴─────┐
      │           │
      ▼           ▼
┌──────────┐  ┌──────────┐
│ Accepted │  │ Rejected │
└────┬─────┘  └──────────┘
     │
     │ Reservation expires
     ▼
┌──────────┐
│ Expired  │
└──────────┘
```

### Reachability State Machine

```text
    ┌─────────────────────┐
    │ Unknown Reachability│ ← Initial state
    └──────────┬──────────┘
               │ AutoNAT probe
               │
        ┌──────┴──────┐
        │             │
        ▼             ▼
┌──────────────┐ ┌──────────────┐
│   Private    │ │   Public     │
│ (Behind NAT) │ │ (Reachable)  │
└──────┬───────┘ └──────┬───────┘
       │                │
       │ Actions:       │ Actions:
       │ • Setup relay  │ • Enable AutoNAT server
       │ • Use DCUtR    │ • Register with Rendezvous
       │                │ • Accept connections
       │                │
       └────────────────┘
              │
              │ Address expires/changes
              ▼
       (Re-probe reachability)
```

## Connection Lifecycle

### Outbound Connection

```text
NetworkClient.dial(addr)
        │
        ▼
NetworkMessage::Dial
        │
        ▼
┌───────────────────┐
│ swarm.dial(addr)  │
└─────────┬─────────┘
          │
          ▼
┌───────────────────┐
│ SwarmEvent::      │
│ Dialing{peer_id}  │
└─────────┬─────────┘
          │
    ┌─────┴─────┐
    │           │
    ▼           ▼
Success      Failure
    │           │
    ▼           ▼
┌─────────────────┐  ┌──────────────────────┐
│ConnectionEstab- │  │OutgoingConnectionErr │
│lished           │  └──────────────────────┘
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Update discovery│
│ state           │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Identify        │
│ exchange        │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Check for       │
│ special roles   │
│ (relay/rendez.) │
└─────────────────┘
```

### Inbound Stream

```text
Remote peer calls open_stream()
           │
           ▼
libp2p_stream accepts on protocol
           │
           ▼
┌─────────────────────────────────┐
│ incoming_streams.next()         │
│ yields (peer_id, P2pStream)     │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│ FromIncoming::from_stream(...)  │
│ wraps in Stream type            │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│ StreamHandler::handle()         │
│ dispatches NetworkEvent::       │
│ StreamOpened{peer_id, stream}   │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│ event_dispatcher.dispatch()     │
│ → calimero-node receives stream │
└─────────────────────────────────┘
```

## Message Flow Diagrams

### Gossipsub Publish

```text
Application                NetworkManager               libp2p                  Peers
    │                            │                         │                       │
    │  Publish{topic, data}      │                         │                       │
    │───────────────────────────►│                         │                       │
    │                            │                         │                       │
    │                            │ gossipsub.publish()     │                       │
    │                            │────────────────────────►│                       │
    │                            │                         │                       │
    │                            │                         │  GossipsubMessage     │
    │                            │                         │──────────────────────►│
    │                            │                         │                       │
    │                            │◄────────────────────────│                       │
    │                            │ Ok(MessageId)           │                       │
    │◄───────────────────────────│                         │                       │
    │  Ok(MessageId)             │                         │                       │
```

### Gossipsub Receive

```text
Peers                      libp2p               NetworkManager              Application
  │                           │                        │                         │
  │  GossipsubMessage         │                        │                         │
  │──────────────────────────►│                        │                         │
  │                           │                        │                         │
  │                           │ BehaviourEvent::       │                         │
  │                           │ Gossipsub(Message{})   │                         │
  │                           │───────────────────────►│                         │
  │                           │                        │                         │
  │                           │                        │ NetworkEvent::Message   │
  │                           │                        │────────────────────────►│
  │                           │                        │                         │
  │                           │                        │                         │ Process
  │                           │                        │                         │ delta
```

### Stream Sync

```text
Initiator                  NetworkManager               Responder's NM           Responder
    │                            │                            │                      │
    │  OpenStream(peer_id)       │                            │                      │
    │───────────────────────────►│                            │                      │
    │                            │                            │                      │
    │                            │ stream.open_stream()       │                      │
    │                            │───────────────────────────►│                      │
    │                            │                            │                      │
    │                            │                            │ StreamOpened event   │
    │                            │                            │─────────────────────►│
    │                            │                            │                      │
    │                            │◄───────────────────────────│                      │
    │◄───────────────────────────│ Stream                     │                      │
    │  Ok(Stream)                │                            │                      │
    │                            │                            │                      │
    │────────────────────────────┼──── Sync Messages ────────┼─────────────────────►│
    │◄───────────────────────────┼──── Sync Messages ────────┼──────────────────────│
    │                            │                            │                      │
```

## Design Decisions

### 1. Actor Model (Actix)

**Decision**: Use Actix actor for `NetworkManager` instead of raw async tasks.
*(See: `src/lib.rs:NetworkManager`)*

**Rationale**:
- Simplified concurrency: Single-threaded actor avoids lock contention
- Message-based interface: Clean separation between network and application
- Built-in mailbox: Backpressure handling for commands
- Matches `calimero-node`'s architecture (also uses Actix)

### 2. Event Dispatcher Trait

**Decision**: Use `NetworkEventDispatcher` trait instead of Actix `Recipient`.
*(See: `primitives/src/messages.rs:NetworkEventDispatcher`)*

**Rationale**:
- Flexibility: Can use channels, direct calls, or Actix recipients
- Testability: Easy to mock for unit tests
- Decoupling: Network crate doesn't depend on how events are consumed

### 3. Composed Behaviour

**Decision**: Single composed `Behaviour` struct with all protocols.
*(See: `src/behaviour.rs:Behaviour`)*

**Rationale**:
- libp2p pattern: This is the recommended approach
- Automatic event handling: `NetworkBehaviour` derive handles event routing
- Shared state: All behaviours see the same connection state

### 4. Discovery State Separation

**Decision**: Maintain `DiscoveryState` separate from libp2p's internal state.
*(See: `src/discovery/state.rs:DiscoveryState`)*

**Rationale**:
- Additional metadata: Track rendezvous registration status, relay reservations
- Protocol-level info: Store supported protocols per peer (from Identify)
- Persistence: Can keep peer info after disconnection for reconnection

### 5. Primitives Crate

**Decision**: Separate `calimero-network-primitives` crate.
*(See: `primitives/` directory)*

**Rationale**:
- Avoid circular dependencies: Other crates can use types without full network dep
- Lighter compilation: Primitives compile faster than full network crate
- API stability: Primitives change less frequently than implementation

### 6. Stream Framing

**Decision**: Length-delimited framing with 8MB max message size.
*(See: `primitives/src/stream/codec.rs:MessageCodec`)*

**Rationale**:
- Simplicity: Standard approach, well-tested
- Flexibility: Any serialization format can be used inside
- Safety: Prevents memory exhaustion from large messages
- Compatibility: Works with any transport (TCP, QUIC, relay)

### 7. Custom Kademlia Protocol

**Decision**: Use `/calimero/kad/1.0.0` instead of IPFS DHT.
*(See: `src/behaviour.rs:CALIMERO_KAD_PROTO_NAME`)*

**Rationale**:
- Network isolation: Calimero nodes form separate DHT
- Bootstrap control: Use Calimero-specific bootstrap nodes
- Protocol evolution: Can modify DHT behavior independently
