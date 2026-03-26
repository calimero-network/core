# Issue 1: Gossipsub mesh wait too short for uninitialized nodes

## Problem

When a node joins a context, the gossipsub mesh for that context's topic takes multiple heartbeats (5-10s on localhost, potentially 30s+ over relay connections) to form. The sync manager only waits 1.5s (3 retries × 500ms) before giving up with "No peers to sync with". An uninitialized node that can't find mesh peers cannot initiate snapshot sync and stays permanently uninitialized.

## Reproduction

```bash
cd apps/sync-test
# Start 3 nodes, have Node 1 create context + write data
# Kill Node 3, then restart and join
# Node 3 stays at root_hash=[0;32] — never syncs
```

Reproduced locally with merobox workflow and manual `run-nodes.sh` kill/restart.

## Root Cause

`perform_interval_sync` in `crates/node/src/sync/manager.rs`:

```rust
// Only 3 retries × 500ms = 1.5s — not enough
for attempt in 1..=3 {
    peers = self.network_client.mesh_peers(TopicHash::from_raw(context_id)).await;
    if !peers.is_empty() { break; }
    time::sleep(std::time::Duration::from_millis(500)).await;
}
```

## Fix

For uninitialized nodes (`root_hash == [0;32]`), increase to 10 retries × 1s = 10s. For initialized nodes, keep the existing 3 × 500ms.

Additionally: for production over relay connections where mesh may take 30s+, consider a fallback that uses `open_stream` to any connected peer (bypassing gossipsub mesh entirely) when the node is uninitialized.

## Files

- `crates/node/src/sync/manager.rs` — `perform_interval_sync`

## Tests

- `apps/sync-test/workflows/three-node-sync.yml` — Phase 3 (Sandi joins after existing writes)
- `apps/sync-test/workflows/six-node-sync.yml` — Phase 3 (3 late joiners)
- Manual: `apps/sync-test/run-nodes.sh` kill/restart scenario
