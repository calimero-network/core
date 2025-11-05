# calimero-protocols

Stateless network protocol handlers for Calimero node.

## Status: 🚧 WORK IN PROGRESS (Week 1 - 50% Complete)

**What Works** ✅:
- **SecureStream** (1,084 lines) - Fully functional authentication & encryption
- Crate compiles & tests pass

**What's Left** ⏳:
- key_exchange (185 lines) - 95% done, minor import issues
- delta_request (420 lines) - Needs stateless refactoring
- blob_request (263 lines) - Needs stateless refactoring
- state_delta (765 lines) - Needs porting + refactoring

**Estimated**: 6-8 hours to complete Week 1

---

## Architecture

This crate provides **stateless protocol handlers** - no hidden state, all dependencies injected.

### Structure

```
src/
├── lib.rs
├── gossipsub/          # Broadcast protocols (one-to-many)
│   └── state_delta.rs  # Process state change broadcasts
├── p2p/                # Request/response protocols (one-to-one)
│   ├── delta_request.rs   # Fetch specific delta
│   ├── blob_request.rs    # Fetch blob
│   └── key_exchange.rs    # Exchange encryption keys
└── stream/             # Secure stream utilities
    ├── authenticated.rs   # SecureStream (challenge-response auth)
    ├── helpers.rs         # Private send/recv (ENFORCES auth!)
    └── tracking.rs        # Sequencer, SyncState
```

###Design Principles

1. **Stateless**: All state injected as parameters (testable!)
2. **No actors**: Plain async Rust
3. **Secure by default**: helpers are pub(crate) - can't bypass auth
4. **Reusable**: Not coupled to node runtime

---

## What's Different from Old Code

**Old** (node/sync/):
```rust
impl SyncManager {
    pub async fn handle_delta_request(&self, ...) {
        // Hidden deps: self.context_client, self.config
        // Tightly coupled to SyncManager
        // Hard to test
    }
}
```

**New** (protocols/):
```rust
pub async fn handle_delta_request(
    stream: &mut SecureStream,
    delta_id: [u8; 32],
    delta_store: &DeltaStore,  // Injected!
    context_client: &ContextClient,  // Injected!
) -> Result<()> {
    // Pure function
    // All deps explicit
    // Easy to test!
}
```

---

## Progress Tracking

**Week 1** (calimero-protocols): 50% complete
- ✅ SecureStream (3 hrs) - DONE
- ✅ key_exchange (2 hrs) - 95% done
- ⏳ delta_request (3 hrs) - Needs refactoring
- ⏳ blob_request (2 hrs) - Needs refactoring
- ⏳ state_delta (3 hrs) - Needs porting

**Week 2-4**: calimero-sync, calimero-node runtime, migration

---

## Current Milestone

🎉 **SecureStream is WORKING and SECURE BY DEFAULT!**

This alone is massive progress - we can now build P2P protocols knowing they'll be secure.

Next: Finish refactoring the remaining protocols (6-8 hours of focused work).

