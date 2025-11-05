# E2E Test Mismatch Analysis

## Current Status (After Fixing Uninitialized Errors)

### ✅ Fixed Issues:
- **ALL Uninitialized errors resolved** (was 4-10, now 0!)
- Join context works reliably
- Delta sync works (parent fetching with fresh streams)
- Metadata updates work (context metadata persisted correctly)

### ❌ Remaining Issues (9 failures):

1. `"expected: \"\" actual: \"Hello\""` - Extra state
2. `"expected: \"synced\" actual: null"` - Missing flag
3. `"expected: [{message}] actual: []"` - Empty messages array  
4. `"expected: \"test_value\" actual: null"` - Missing value
5. `"expected: \"baz\" actual: \"bar\""` - Old value
6. `"expected: 2 actual: 0"` - Counter at 0
7. `"expected: 5 actual: 4"` - Counter off by 1
8. `500 Internal Server Error` - One join failing
9. `"expected: 3 actual: 2"` - Counter off by 1

## Delta Application Statistics:

```
Node1: 29 deltas applied
Node2: 9 deltas applied  ⚠️ ONLY 31% of deltas!
Node3: 29 deltas applied
```

## Broadcast Statistics:

```
Node1: 16 broadcasts received
Node2: 4 broadcasts received  ⚠️ ONLY 25% of broadcasts!
Node3: 16 broadcasts received
```

## Root Cause Hypothesis:

**Node2 is missing most gossipsub broadcasts!**

### Potential Causes:

1. **Gossipsub mesh not forming properly for node2**
   - Other nodes subscribe to node2's topics
   - But node2 might not be in the mesh for topics it should receive

2. **Subscription timing issue**
   - Node2 creates contexts (doesn't need to join)
   - But when OTHER nodes modify those contexts, broadcasts might not reach node2
   - Gossipsub mesh might not include node2 if it didn't explicitly subscribe

3. **Node2 as inviter doesn't subscribe to its own contexts**
   - When creating a context, does node2 call `subscribe()`?
   - Joiners call `subscribe()` in `join_context`, but creators might not

## Next Steps:

1. Check if context creators call `subscribe()` to their own topics
2. Verify gossipsub mesh includes all nodes for each context
3. Add logging to show which nodes are in mesh when broadcasting
4. Check if broadcasts are being sent to all mesh peers

## Quick Fix Hypothesis:

The creator probably needs to explicitly subscribe to the gossipsub topic when creating a context,
similar to how joiners subscribe in `join_context`.

