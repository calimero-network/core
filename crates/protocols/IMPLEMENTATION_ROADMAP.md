# calimero-protocols Implementation Roadmap

This document tracks the implementation of the clean protocols crate.

---

## Week 1 Progress

### ✅ Foundation Complete

- [x] Create crate structure
- [x] Add to workspace
- [x] Create module stubs (compiles with TODOs)
- [x] Define public API in lib.rs

### ⏳ Implementation Tasks

#### Task 1: Implement AuthenticatedStream (~2-3 hours)

**Goal**: Merge `stream.rs` + `secure_stream.rs` into ONE always-secure API

**Source Files to Merge**:
- `node/src/sync/stream.rs` (85 lines) - send/recv logic
- `node/src/sync/secure_stream.rs` (856 lines) - authentication logic

**Target**: `protocols/src/stream/authenticated.rs`

**Approach**:
1. Copy authentication logic from `secure_stream.rs`
2. Copy send/recv from `stream.rs`
3. Make send/recv private methods (called by public AuthenticatedStream)
4. Remove all direct stream::send() calls (force authentication)

**Test Plan**:
- Unit test: Challenge-response flow
- Unit test: Encryption/decryption
- Unit test: Nonce rotation
- Integration test: Full authentication + message exchange

---

#### Task 2: Port Gossipsub State Delta (~3-4 hours)

**Goal**: Make `state_delta` handler stateless and testable

**Source**: `node/src/handlers/state_delta.rs` (765 lines)

**Target**: `protocols/src/gossipsub/state_delta.rs`

**Changes Needed**:
1. Extract validation logic (decrypt, deserialize)
2. Make DeltaStore a parameter (not owned)
3. Make ContextClient a parameter (not owned)
4. Return structured result (applied, cascaded, events)
5. Remove actor dependencies
6. Add comprehensive tests

**Function Signature**:
```rust
pub struct StateDeltaParams<'a> {
    pub delta_id: [u8; 32],
    pub parents: Vec<[u8; 32]>,
    pub artifact: Vec<u8>,
    pub author_id: PublicKey,
    pub sender_key: PrivateKey,
    pub nonce: Nonce,
    pub expected_root_hash: [u8; 32],
    pub events: Option<Vec<u8>>,
    
    // Injected dependencies
    pub delta_store: &'a DeltaStore,
    pub context_client: &'a ContextClient,
    pub our_identity: PublicKey,
}

pub async fn handle_state_delta(params: StateDeltaParams<'_>) -> Result<StateDeltaResult>
```

**Test Plan**:
- Unit test: Valid delta (applied immediately)
- Unit test: Delta with missing parents (goes pending)
- Unit test: Delta decryption failure
- Unit test: Event deserialization
- Mock: DeltaStore for isolated testing

---

#### Task 3: Port P2P Delta Request (~2 hours)

**Source**: `node/src/sync/delta_request.rs` (420 lines)

**Target**: `protocols/src/p2p/delta_request.rs`

**Split Into**:
```rust
// Server side
pub async fn handle_delta_request(
    stream: &mut AuthenticatedStream,
    delta_id: [u8; 32],
    delta_store: &DeltaStore,
) -> Result<()>

// Client side
pub async fn request_delta(
    peer: PeerId,
    context_id: &ContextId,
    delta_id: [u8; 32],
    network_client: &NetworkClient,
    context_client: &ContextClient,
) -> Result<CausalDelta<Vec<Action>>>
```

**Test Plan**:
- Unit test: Request/response flow
- Unit test: Delta not found handling
- Unit test: Network error handling
- Mock: Network + DeltaStore

---

#### Task 4: Port P2P Blob Request (~2 hours)

**Source**: `node/src/sync/blobs.rs` (263 lines)

**Target**: `protocols/src/p2p/blob_request.rs`

**Split Into**:
```rust
// Server side
pub async fn handle_blob_request(
    stream: &mut AuthenticatedStream,
    blob_id: BlobId,
    blobstore: &BlobManager,
) -> Result<()>

// Client side  
pub async fn request_blob(
    peer: PeerId,
    blob_id: BlobId,
    network_client: &NetworkClient,
    context_client: &ContextClient,
) -> Result<Vec<u8>>
```

**Test Plan**:
- Unit test: Request/response flow
- Unit test: Blob not found handling
- Unit test: Large blob streaming
- Mock: Blobstore

---

#### Task 5: Port P2P Key Exchange (~2 hours)

**Source**: `node/src/sync/key.rs` (113 lines)

**Target**: `protocols/src/p2p/key_exchange.rs`

**Already uses SecureStream** (good!), just need to:
1. Make stateless (parameters instead of self.context_client)
2. Remove manager coupling
3. Add tests

**Test Plan**:
- Unit test: Bidirectional key exchange
- Unit test: Authentication failures
- Mock: ContextClient

---

## Success Criteria for Week 1

- [x] Crate compiles
- [ ] All TODOs replaced with real implementations
- [ ] All protocols have tests
- [ ] Code coverage > 80%
- [ ] Documentation complete
- [ ] No dependencies on old node code

---

## Timeline

**Day 1-2**: AuthenticatedStream (merge + test)
**Day 3**: Gossipsub state_delta (port + test)
**Day 4**: P2P delta_request (port + test)
**Day 5**: P2P blob_request + key_exchange (port + test)

**Estimated**: 5 days for complete, tested protocols crate

---

## After Week 1

Once protocols crate is complete:
- ✅ Protocols testable in isolation
- ✅ Can be reused in other contexts
- ✅ Secure by default (no way to bypass auth)
- ✅ Clear API (no framework coupling)

**Then**: Start Week 2 (calimero-sync crate)

