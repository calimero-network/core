# Issue 003: Sync Handshake Protocol Messages

**Priority**: P0 (Foundation)  
**CIP Section**: §2 - Sync Handshake Protocol

## Summary

Implement the `SyncHandshake` and `SyncHandshakeResponse` messages that enable protocol negotiation between peers.

## Wire Protocol Messages

### SyncHandshake (Initiator → Responder)

```rust
pub struct SyncHandshake {
    /// Protocol version for compatibility
    pub version: u32,
    
    /// Our current Merkle root hash
    pub root_hash: [u8; 32],
    
    /// Number of entities in our tree
    pub entity_count: usize,
    
    /// Maximum depth of our Merkle tree
    pub max_depth: usize,
    
    /// DAG heads (latest delta IDs)
    pub dag_heads: Vec<[u8; 32]>,
    
    /// Whether we have any state
    pub has_state: bool,
    
    /// Protocols we support (ordered by preference)
    pub supported_protocols: Vec<SyncProtocol>,
}
```

### SyncHandshakeResponse (Responder → Initiator)

```rust
pub struct SyncHandshakeResponse {
    /// Agreed protocol for this sync session
    pub selected_protocol: SyncProtocol,
    
    /// Responder's root hash
    pub root_hash: [u8; 32],
    
    /// Responder's entity count
    pub entity_count: usize,
    
    /// Responder's capabilities
    pub capabilities: SyncCapabilities,
}
```

### SyncCapabilities

```rust
pub struct SyncCapabilities {
    pub supports_compression: bool,
    pub max_batch_size: usize,
    pub supported_protocols: Vec<SyncProtocol>,
}
```

## Implementation Tasks

- [ ] Define message structs in `crates/node/primitives/src/sync.rs`
- [ ] Implement Borsh serialization
- [ ] Add version field for future compatibility
- [ ] Implement `SyncHandshake::new()` helper
- [ ] Implement `SyncHandshakeResponse::from_handshake()` helper
- [ ] Add request/response handling in network layer

## Wire Protocol Version

Start at version `1`. Increment on breaking changes.

```rust
pub const SYNC_PROTOCOL_VERSION: u32 = 1;
```

## Acceptance Criteria

- [ ] Handshake messages serialize/deserialize correctly
- [ ] Version mismatch is detected gracefully
- [ ] Capability negotiation selects common protocols
- [ ] Unit tests for all message types

## Files to Modify

- `crates/node/primitives/src/sync.rs` (new)
- `crates/node/primitives/src/lib.rs`
- `crates/network/src/stream/message.rs`

## POC Reference

See Phase 3 (Network Layer) in [POC-IMPLEMENTATION-NOTES.md](../POC-IMPLEMENTATION-NOTES.md)
