# Calimero Protocols - Network Communication

> **Stateless, testable P2P and broadcast protocols for distributed nodes**

Clean protocol implementations with zero framework coupling. All handlers are pure functions that take their dependencies as parameters - no hidden state, no actors, just async Rust.

---

## Quick Start

```rust
use calimero_protocols::p2p::key_exchange;
use calimero_protocols::SecureStream;

// Request key exchange with a peer (stateless - all deps injected!)
key_exchange::request_key_exchange(
    &network_client,
    &context,
    our_identity,
    peer_id,
    &context_client,
    Duration::from_secs(10),
).await?;

// Handle incoming key exchange (server side)
key_exchange::handle_key_exchange(
    stream,
    &context,
    our_identity,
    their_identity,
    their_nonce,
    &context_client,
    timeout,
).await?;
```

**What you get:**
- ✅ **Zero hidden dependencies** - all state injected as parameters
- ✅ **Fully testable** - no infrastructure needed
- ✅ **Secure by default** - all P2P uses authenticated streams
- ✅ **No actors** - plain async Rust

---

## Protocol Categories

### P2P Protocols (Request/Response)

One-to-one authenticated communication between peers.

| Protocol | Purpose | Security |
|----------|---------|----------|
| **key_exchange** | Exchange encryption keys | Mutual authentication |
| **delta_request** | Request missing DAG deltas | Context member verification |
| **blob_request** | Request application blobs | Context member verification |
| **blob_protocol** | Download blobs (public) | None (CALIMERO_BLOB_PROTOCOL) |

### Gossipsub Protocols (Broadcast)

One-to-many encrypted broadcasts within a context.

| Protocol | Purpose | Processing |
|----------|---------|------------|
| **state_delta** | Broadcast state changes | DAG cascade + apply |

---

## Architecture

```
Your Code
    ↓
┌─────────────────────────────────────────┐
│ Stateless Protocol Handlers             │
│ • All dependencies injected             │
│ • Pure async functions                  │
│ • No framework coupling                 │
└─────────────────────────────────────────┘
    ↓
┌─────────────────────────────────────────┐
│ SecureStream (Authentication Layer)     │
│ • Challenge-response auth               │
│ • Mutual identity verification          │
│ • Encrypted messaging                   │
└─────────────────────────────────────────┘
    ↓
┌─────────────────────────────────────────┐
│ Network Layer (libp2p)                  │
│ • Stream transport                      │
│ • Peer discovery                        │
│ • Connection management                 │
└─────────────────────────────────────────┘
```

---

## Security Model

All P2P protocols use `SecureStream` for authentication:

```rust
// Automatic mutual authentication
SecureStream::authenticate_p2p(
    stream,
    context,
    our_identity,
    &context_client,
    timeout,
).await?;
// ✅ Both peers verified
// ✅ sender_keys exchanged
// ✅ Ready for encrypted communication
```

**Security guarantees:**
- ✅ Challenge-response prevents impersonation
- ✅ Mutual authentication (both sides verified)
- ✅ Context membership validation
- ✅ Encrypted message passing
- ✅ Nonce rotation prevents replay attacks

---

## Common Patterns

### Pattern 1: Request/Response with Auth

```rust
// Client: Open stream + authenticate + request
let mut stream = network_client.open_stream(&peer_id).await?;
SecureStream::authenticate_p2p(&mut stream, ...).await?;
send_request(&mut stream, request_data).await?;
let response = receive_response(&mut stream).await?;
```

### Pattern 2: Server: Receive + Verify + Respond

```rust
// Server: Already have stream from handler
SecureStream::verify_identity(
    &mut stream,
    &context,
    our_identity,
    &context_client,
    timeout,
).await?;
// ✅ Peer is verified context member
let request = receive_request(&mut stream).await?;
send_response(&mut stream, response_data).await?;
```

### Pattern 3: Gossipsub Broadcast

```rust
// No authentication needed (encrypted at transport layer)
state_delta::handle_state_delta(
    &node_client,
    &context_client,
    &network_client,
    &delta_store,
    our_identity,
    timeout,
    // ... delta fields ...
).await?;
```

---

## Testing

All protocols are designed for easy testing:

```rust
#[tokio::test]
async fn test_key_exchange() {
    // No infrastructure needed!
    let (client_stream, server_stream) = create_connected_streams();
    let context = create_test_context();
    
    // Run both sides concurrently
    tokio::try_join!(
        key_exchange::request_key_exchange(...),
        key_exchange::handle_key_exchange(...),
    )?;
    
    // Verify sender_keys were exchanged
    assert!(context_client.get_identity(...).sender_key.is_some());
}
```

See `tests/` directory for complete examples.

---

## API Reference

### SecureStream

```rust
// Mutual authentication (both send Init first)
SecureStream::authenticate_p2p(
    stream, context, our_identity, context_client, timeout
) -> Result<()>

// Verify peer identity (access control)
SecureStream::verify_identity(
    stream, context, our_identity, context_client, timeout
) -> Result<()>

// Prove our identity (respond to challenge)
SecureStream::prove_identity(
    stream, context, our_identity, context_client, timeout
) -> Result<()>
```

### P2P: key_exchange

```rust
// Client side
request_key_exchange(
    network_client, context, our_identity, peer_id, context_client, timeout
) -> Result<()>

// Server side
handle_key_exchange(
    stream, context, our_identity, their_identity, their_nonce,
    context_client, timeout
) -> Result<()>
```

### P2P: delta_request

```rust
// Request missing deltas to fill DAG gaps
request_delta(
    network_client, context_id, our_identity, peer, missing_delta_id,
    context_client, timeout
) -> Result<CausalDelta>

// Handle delta request from peer
handle_delta_request(
    stream, context, our_identity, requested_id, delta_store,
    context_client, timeout
) -> Result<()>
```

### Gossipsub: state_delta

```rust
// Handle broadcasted state delta
handle_state_delta(
    node_client, context_client, network_client, delta_store,
    our_identity, timeout, source, context_id, author_id, delta_id,
    parent_ids, hlc, root_hash, artifact, nonce, events
) -> Result<()>
```

---

## Performance Characteristics

| Operation | Latency | Notes |
|-----------|---------|-------|
| **Local protocol call** | < 1ms | Pure async, no blocking |
| **Authentication** | ~50ms | One RTT + crypto |
| **Delta request** | ~100ms | Auth + delta fetch |
| **Gossipsub delivery** | ~10ms | No auth (pre-encrypted) |

Network latency dominates - protocol overhead is negligible.

---

## Design Principles

1. **Stateless** - All handlers are pure functions
2. **Testable** - No infrastructure dependencies
3. **Secure by default** - All P2P requires authentication
4. **No framework coupling** - Plain async Rust
5. **Explicit dependencies** - All injected as parameters

---

## FAQ

**Q: Why are all protocols stateless?**  
A: Testability and composability. No hidden state = easy to test, easy to understand.

**Q: Do I need to call SecureStream manually?**  
A: Not for gossipsub (encrypted at transport). For P2P, yes - security requires explicit verification.

**Q: Can I use these protocols outside the node?**  
A: Yes! That's the point. Just inject the dependencies you need.

**Q: What about connection pooling?**  
A: Handled by network layer (libp2p). Protocols just use the streams.

---

## License

See root [LICENSE](../../LICENSE) file.
