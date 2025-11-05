# CRITICAL: Sync Implementation TODO

## Problem

`sync_and_wait()` is currently broken, which causes **critical state divergence** for nodes joining existing contexts.

### Current Broken Behavior

1. Node joins context with existing state (10 deltas already exist)
2. Node subscribes to gossipsub topic ✅
3. Node receives ONLY future deltas (new transactions) ✅  
4. Node NEVER receives historical deltas ❌
5. **Result: Permanent state divergence** - new member is missing all historical state!

### Why Gossipsub Alone Isn't Enough

- Gossipsub broadcasts NEW deltas only
- Historical deltas are NOT rebroadcast
- New members join with empty DAG
- They only get future transactions
- **They permanently miss all past state!**

---

## Required Fix

Implement proper DAG catchup using existing protocols.

### Architecture

```
join_context() needs:
    ↓
sync_and_wait() should:
    ↓
1. Find peers (network_client.get_peers_for_context())
2. Pick a peer (random or specific inviter)
3. Request their DAG heads via P2P
4. Compare with our heads (empty for new member)
5. Request missing deltas using calimero-protocols::p2p::delta_request
6. Apply deltas to DeltaStore
7. Verify we have full history
```

### Current Blockers

**`NodeClient` doesn't have required dependencies:**

```rust
pub struct NodeClient {
    datastore: Store,           // ✅ Has
    blobstore: BlobManager,     // ✅ Has
    network_client: NetworkClient,  // ✅ Has
    // ❌ Missing: ContextClient (need for get_context, get_members)
    // ❌ Missing: DeltaStore (need for get_heads, add_delta)
}
```

**Can't implement sync without:**
- `ContextClient` - to get context info, member list
- `DeltaStore` - to get our heads, add fetched deltas
- Access to `calimero-sync` strategies

---

## Solution Options

### Option 1: Add Dependencies to NodeClient (Quick Fix)

```rust
pub struct NodeClient {
    // ... existing fields ...
    context_client: ContextClient,  // Add this
    // DeltaStore access via NodeManager message?
}

impl NodeClient {
    pub async fn sync_and_wait(...) -> Result<SyncResult> {
        // 1. Get context
        let context = self.context_client.get_context(context_id)?;
        
        // 2. Get members (find peers)
        let members = self.context_client.get_context_members(context_id)?;
        let peer = pick_random_peer(&members);
        
        // 3. Get our heads (need DeltaStore access!)
        let our_heads = ???; // How to get DeltaStore?
        
        // 4. Request their heads
        let their_heads = request_dag_heads(&peer).await?;
        
        // 5. Find missing deltas
        let missing = compute_missing(our_heads, their_heads);
        
        // 6. Fetch missing deltas
        for delta_id in missing {
            let delta = calimero_protocols::p2p::delta_request::request_delta(
                &self.network_client,
                context_id,
                our_identity,
                peer,
                delta_id,
                &self.context_client,
                timeout,
            ).await?;
            
            // 7. Add delta (need DeltaStore!)
            ???.add_delta(delta).await?;
        }
        
        Ok(SyncResult::DeltaSync)
    }
}
```

**Problems:**
- Still need DeltaStore access (not available in NodeClient)
- Circular dependency risk (NodeClient → ContextClient → NodeClient)

### Option 2: Implement at NodeManager Level (Proper Fix)

Move sync logic to where we have all dependencies:

```rust
impl NodeManager {
    async fn handle_sync_request(
        &mut self,
        context_id: ContextId,
        peer_id: Option<PeerId>,
    ) -> Result<SyncResult> {
        // We have everything here:
        // - self.clients.context (ContextClient)
        // - self.state.delta_stores (DeltaStore)
        // - self.managers.network (NetworkClient)
        
        // Get delta store
        let delta_store = self.state.delta_stores
            .get(&context_id)
            .ok_or_else(|| eyre!("Context not found"))?;
        
        // Get our identity
        let our_identity = self.clients.context
            .get_our_identity(&context_id)?;
        
        // Get members to find peers
        let members = self.clients.context
            .get_context_members(&context_id, Some(true))?;
        
        // Pick peer (or use provided peer_id)
        let peer = peer_id.or_else(|| pick_random_peer(&members));
        
        // Use calimero-sync strategy
        let strategy = calimero_sync::strategies::DagCatchup::new(
            self.managers.network.clone(),
            self.clients.context.clone(),
            Duration::from_secs(10),
        );
        
        // Execute sync
        let result = strategy.sync(
            &context_id,
            &peer,
            &our_identity,
            &**delta_store,
        ).await?;
        
        Ok(result)
    }
}
```

**Then wire it up:**

```rust
// In NodeManager Actor implementation
impl Handler<SyncRequest> for NodeManager {
    type Result = ResponseFuture<Result<SyncResult>>;
    
    fn handle(&mut self, msg: SyncRequest, _ctx: &mut Context<Self>) -> Self::Result {
        let (context_id, peer_id, result_tx) = msg;
        // ... implementation ...
    }
}

// NodeClient sends message to NodeManager
impl NodeClient {
    pub async fn sync_and_wait(...) -> Result<SyncResult> {
        self.node_manager.send(SyncRequest { ... }).await?
    }
}
```

**Benefits:**
- ✅ All dependencies available
- ✅ Can use calimero-sync strategies
- ✅ Proper separation of concerns
- ✅ Testable (can mock NodeManager)

**Drawbacks:**
- Requires actor message passing (but NodeManager is already an actor)
- More code changes

### Option 3: Implement Sync Actor (Clean Separation)

Create dedicated `SyncActor` to handle all sync operations:

```rust
pub struct SyncActor {
    context_client: ContextClient,
    network_client: NetworkClient,
    delta_stores: Arc<DeltaStoreService>,
    config: SyncConfig,
}

impl Actor for SyncActor {
    type Context = actix::Context<Self>;
}

impl Handler<SyncRequest> for SyncActor {
    // ... implementation using calimero-sync ...
}
```

**Benefits:**
- ✅ Clean separation of concerns
- ✅ Dedicated sync responsibility
- ✅ Easy to test independently
- ✅ Can use calimero-sync strategies

**Drawbacks:**
- Another actor to manage
- Need to wire into NodeManager startup

---

## Recommended Approach

**Option 2: Implement at NodeManager Level**

Why:
1. NodeManager already has all dependencies
2. No new actors needed
3. Can reuse calimero-sync strategies
4. Minimal architectural changes

### Implementation Steps

1. **Add SyncRequest handler to NodeManager**
   - Listen on the `ctx_sync_rx` channel we already created
   - Handle sync requests with full access to dependencies

2. **Implement sync logic**
   - Use `calimero-sync::strategies::DagCatchup`
   - Request DAG heads from peer
   - Fetch missing deltas
   - Apply to DeltaStore

3. **Wire up NodeClient**
   - `sync_and_wait()` sends request via `ctx_sync_tx`
   - NodeManager processes it
   - Returns result via oneshot channel

4. **Test**
   - Unit test sync logic
   - E2E test join_context with existing state

---

## Temporary Workaround

Until proper sync is implemented:

**For E2E tests:**
- Tests work IF context is empty when joining
- Tests work IF new transactions happen after join
- Tests FAIL if joining context with existing state and no new activity

**For production:**
- ⚠️ **DO NOT USE** - sync is broken
- New members will miss historical state
- Permanent divergence risk

---

## Timeline

**Immediate (this session):**
- ❌ Can't implement full solution (too complex)
- ✅ Document the problem clearly
- ✅ Add warnings in code
- ✅ Commit with clear TODO

**Next session:**
- Implement Option 2 (NodeManager sync handler)
- Wire up calimero-sync strategies
- Test with join_context scenarios
- Remove warnings once working

---

## Testing Strategy

Once implemented, test these scenarios:

1. **Empty context join** - should work immediately
2. **Join with existing state** - should fetch all historical deltas
3. **Join + immediate execute** - should see full state before executing
4. **Join with no peers online** - should timeout gracefully
5. **Join with malicious peer** - should validate deltas

---

## Files to Modify

1. `crates/node/src/lib.rs` - NodeManager sync handler
2. `crates/node/src/run.rs` - Start listening on ctx_sync_rx
3. `crates/node/primitives/src/client.rs` - Remove temporary workaround
4. `crates/context/src/handlers/join_context.rs` - Remove warnings

---

## References

- `calimero-sync::strategies::DagCatchup` - sync strategy
- `calimero-protocols::p2p::delta_request` - fetch deltas
- Old `crates/node/src/sync/manager.rs` (deleted) - had working sync implementation

