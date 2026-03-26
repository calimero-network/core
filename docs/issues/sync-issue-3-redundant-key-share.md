# Issue 3: Key share runs unconditionally every sync cycle

## Problem

`initiate_sync_inner` calls `initiate_key_share_process` on every sync cycle (every 10s), even though the `sender_key` is persisted to the DB after the first successful exchange. The key share is an 8-message protocol (~200ms on healthy connections, 10s timeout on broken ones) that gates ALL subsequent sync work (blob share, DAG heads, snapshot, hash comparison).

Combined with Issue 2 (NEAR RPC blocking), this means:
- **Healthy peers**: 200ms wasted per cycle per peer
- **Relay/broken peers**: 10s wasted per cycle before fallback to next peer
- **New members**: Key share fails because receiver blocks on NEAR RPCs, so the member cache never populates, so key share fails again next cycle — permanent loop

## Reproduction

Observed in production and in local testing. Every sync cycle shows:
```
Initiating key share → Peer authenticated → Key share completed  (200ms)
...10s later, same thing...
```

With broken peer `12D3KooWK1jm...`:
```
Initiating key share → 10s timeout → Sync attempt failed → try next peer
```

## Root Cause

`initiate_sync_inner` in `crates/node/src/sync/manager.rs`:

```rust
// Line ~1321: No check for cached sender_key — runs every time
self.initiate_key_share_process(&mut context, our_identity, &mut stream)
    .await
    .wrap_err("key share")?;
```

The `sender_key` IS persisted (line 430-431 of `key.rs`):
```rust
self.context_client.update_identity(&context.id, &their_identity_record)?;
```

But there's no check to skip key share if the key is already cached.

## Fix

Before initiating key share, check if we already have `sender_key` for known members of this context:

```rust
// Pseudocode for the check
let needs_key_share = self.context_client
    .get_context_members(&context.id, Some(false))  // all members
    .any(|(member_id, _)| {
        self.context_client
            .get_identity(&context.id, &member_id)
            .map(|id| id.sender_key.is_none())
            .unwrap_or(true)
    });

if !needs_key_share {
    // Skip key share — we already have all sender_keys
} else {
    self.initiate_key_share_process(...).await?;
}
```

Additionally: track peers that consistently fail key share and temporarily skip them (5-minute TTL blacklist) to avoid wasting 10s on broken/relay peers.

## Files

- `crates/node/src/sync/manager.rs` — `initiate_sync_inner` (line ~1321)
- `crates/context/primitives/src/client/crypto.rs` — `ContextIdentity.sender_key`

## Impact

- Saves ~200ms per sync cycle per peer in steady state
- Saves 10s per cycle when a broken/relay peer is in the mesh
- Prevents the permanent failure loop where key share timeout → no cache update → key share timeout

## Tests

- Merobox: verify sync cycle time drops after first successful key share
- Manual: monitor logs for "Initiating key share" frequency — should only appear on first encounter
