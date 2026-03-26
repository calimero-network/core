# Issue 2: Key share blocks on NEAR RPCs in stream handler

## Problem

When a node receives a key share request from a new member, `internal_handle_opened_stream` calls `sync_context_config` inline if the member isn't in the local cache. This does 3+ NEAR view calls (via relayer or direct RPC), blocking the stream handler for potentially >10s. The initiator's key share `recv` timeout is 10s (`timeout/3`), so the Init ack is never sent in time.

This was the primary cause of the Feb 20 Sandi/Matea/Fran failure: when Sandi (a new member) tried to sync, the receiving peers needed to verify Sandi's membership on-chain, which took >10s.

## Reproduction

Observed in production logs (Feb 20). The pattern:
```
Sandi: Initiating key share → 10s timeout → error=key share
Peer:  recv Init → has_member=false → sync_context_config (3 NEAR RPCs) → >10s → Sandi already gone
```

## Root Cause

`internal_handle_opened_stream` in `crates/node/src/sync/manager.rs`:

```rust
if !self.context_client.has_member(&context_id, &their_identity)? {
    // THIS BLOCKS: 3+ NEAR RPCs, can take >10s
    self.context_client.sync_context_config(context_id, None).await?;
}
```

## Fix

Replace the full `sync_context_config` (3+ RPCs) with a single lightweight on-chain check:

1. Add `check_member_on_chain(context_id, identity)` to `ContextClient` — single `has_member` NEAR view call (~200ms)
2. If confirmed, add to local cache immediately
3. If the single RPC also fails, fall back to `sync_context_config` but with a shorter timeout
4. Schedule a background full sync for later (non-blocking)

## Files

- `crates/node/src/sync/manager.rs` — `internal_handle_opened_stream` (line ~1925)
- `crates/context/primitives/src/client.rs` — new `check_member_on_chain`, `add_member_to_local_cache`
- `crates/context/primitives/src/client/external/config.rs` — existing `has_member` RPC method

## Tests

- Manual: join a context from a fresh node while monitoring receiver logs
- Merobox: 3-node workflow with delayed join (Phase 3)

## Related

- The initiator side also calls `sync_context_config` at `initiate_sync_inner` (line ~1295), but this happens BEFORE the stream is opened, so it doesn't affect the timeout. However, it adds unnecessary latency to every sync cycle.
