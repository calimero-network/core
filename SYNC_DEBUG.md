# Sync Debug - Why Tests Still Fail

## Current State

`join_context` calls `sync_and_wait()` which:
1. Finds peer from mesh ✅
2. Creates empty DeltaStore ✅
3. Calls DagCatchup strategy ❌
4. DagCatchup calls `get_missing_parents()` on empty DAG
5. Returns empty list (no pending deltas!)
6. Returns `NoSyncNeeded` without fetching anything ❌
7. State remains Uninitialized ❌

## The Fundamental Issue

`DagCatchup` is designed for **filling gaps**, not **initial sync**:

```rust
// DagCatchup strategy:
let missing_result = delta_store.get_missing_parents().await;
if missing_result.missing_ids.is_empty() {
    return Ok(SyncResult::NoSyncNeeded);  // ❌ Empty DAG = no missing parents!
}
```

**Gap filling** (what DagCatchup does):
- You have some deltas: [D1, D2, D4]
- D4 has parent D3 (missing!)
- `get_missing_parents()` returns [D3]
- Request D3 from peer ✅

**Initial sync** (what we need):
- You have ZERO deltas: []
- Peer has deltas: [D1, D2, D3, D4]
- `get_missing_parents()` returns [] (no pending!)
- Returns NoSyncNeeded ❌
- **We never fetch D1-D4!**

## Solutions

### Option 1: Request DAG Heads First (Proper)

```rust
// 1. Request peer's DAG heads
let their_heads = request_dag_heads(peer).await?;

// 2. Add them as "pending" to our DAG
for head in their_heads {
    delta_store.mark_as_missing(head);
}

// 3. Now get_missing_parents() returns those heads!
let missing = delta_store.get_missing_parents().await;

// 4. Request them (which will cascade to their parents)
request_missing_deltas(missing).await?;
```

**Problem**: We don't have `request_dag_heads()` protocol yet!

### Option 2: Check if DAG is Empty

```rust
// In DagCatchup strategy:
let our_heads = delta_store.dag_has_delta_applied(&[0; 32]).await;

if our_heads is empty {
    // Empty DAG - need initial sync
    // Request peer's heads somehow?
}
```

**Problem**: Still need to request peer's heads!

### Option 3: Don't Sync During join_context

```rust
// In join_context:
node_client.subscribe(&context_id).await?;
// That's it! No sync_and_wait()

// State will sync via gossipsub when deltas are broadcast
```

**Problem**: Timing issue - might try to execute before first delta arrives!

### Option 4: Wait for First Delta (Simple!)

```rust
// In join_context:
node_client.subscribe(&context_id).await?;

// Wait a bit for gossipsub to deliver any existing deltas
tokio::time::sleep(Duration::from_millis(500)).await;

// If still empty, that's fine - context IS empty
```

**Problem**: Hacky, but might work!

## Recommended Fix

**Implement a request_dag_heads protocol and use it for initial sync.**

For now, Option 4 (wait for gossipsub) is the quickest workaround.

## Why Gossipsub Alone Doesn't Work

Gossipsub delivers deltas that are BROADCAST (new transactions).
It does NOT deliver historical deltas that already exist!

New member needs:
1. Subscribe to topic ✅
2. **Request historical heads from peer** ❌ (missing!)
3. Fetch those deltas ✅ (have protocol)
4. Apply to DAG ✅

Without step 2, we never know what to request!

