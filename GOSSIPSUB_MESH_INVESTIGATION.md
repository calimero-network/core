# Gossipsub Mesh Formation Investigation

## Problem Identified

**Root Cause**: When a node creates a context (not joins), it subscribes to the gossipsub topic successfully, but the **mesh takes 15-20 seconds to form**. Any broadcasts sent during this window are **silently dropped**.

## Evidence

From e2e test logs (node2 as context creator):

```
17:43:02.7Z  INFO Subscribed to context context_id=ATWMivas...
17:43:03.2Z  INFO Gossipsub mesh state before broadcast mesh_peer_count=0 mesh_peers=[]
17:43:03.2Z  WARN No mesh peers - broadcast skipped
... (10 broadcasts skipped in < 1 second)

17:43:24.2Z  INFO Gossipsub mesh state before broadcast mesh_peer_count=2 mesh_peers=[...] 
✅ First successful broadcast (~21 seconds after subscribe!)
```

**Impact:**
- **17 broadcasts dropped** in e2e tests (all from node2 immediately after context creation)
- Node2 only applied **9 deltas** vs **29** for node1/node3
- Data mismatches across nodes

## Attempted Fix

Added gossipsub mesh polling to `NodeClient::subscribe()`:
- Wait up to 5 seconds for at least one mesh peer after subscribing
- Log "Polling gossipsub mesh" every 100ms
- Either return when mesh forms OR warn if still empty after 5s

**Status**: Fix implemented but not yet verified (test run issue).

## Next Steps

1. ✅ Verify mesh polling code actually executes
2. Test if broadcasts now succeed (no more "broadcast skipped" warnings)
3. If mesh still doesn't form:
   - Increase wait time from 5s to 30s?
   - Use libp2p's `GossipsubEvent::Subscribed` event instead of polling?
   - Queue broadcasts and retry when mesh forms?

## Code Changes

- `crates/node/primitives/src/client.rs`:
  - Added mesh peer polling in `subscribe()` 
  - Added mesh diagnostics in `broadcast()`

