# E2E Test Debugging Status

## Progress Summary

### ✅ Fixed Bugs (Committed)

1. **request_key_share_with_peer placeholder** - Was always failing with bail!("placeholder")
2. **"expected peer identity to exist" errors** - Fixed by syncing context config in handle_key_exchange
3. **Concurrent delta handler optimization** - Added double-check to skip duplicate key exchanges
4. **Removed duplicate Init send** - `request_key_exchange` was sending Init before calling `authenticate_p2p`

### ❌ Remaining Issue

**"expected ChallengeResponse, got Init" (19 errors)**

## Current Test Results

- **7-8 failures** (down from 10 initially, 0 Uninitialized errors!)
- **0 identity errors** (was 23!)
- **19 challenge-response errors** (all in key exchange)
- **0 successful key exchanges**

## Root Cause Analysis (In Progress)

### Observations from Logs

For context `EdXc36...` with node3 (identity=53Y6T6w) → node1:

**Node3 (client)**:
```
20:29:09.206: Initiating key exchange
20:29:09.781: Retry 1
20:29:10.555: Retry 2
20:29:11.115: Retry 3
... (6 total attempts, all fail)
```

**Node1 (server)**:
```
20:29:09.207: Handling key exchange (their_identity=53Y6T6w)
20:29:09.215: Created ghost identity (sync_context_config done)
20:29:09.222: FAIL - expected ChallengeResponse, got Init (15ms after start!)
20:29:09.784: Handling again (retry 1)
20:29:09.799: FAIL again
... (each fails ~7-15ms after starting)
```

### Message Sequence (Expected)

**Client** (`request_key_exchange` → `authenticate_p2p`):
1. Send Init
2. Recv Init ack
3. Start challenge-response (role=initiator or responder based on identity comparison)

**Server** (`handle_key_exchange`):
1. (Init already consumed)
2. Send Init ack
3. Start challenge-response (role=initiator or responder based on identity comparison)

### Hypothesis

The error happens almost immediately (7-15ms), suggesting:
- NOT a timeout or slow network
- NOT multiple retries on same stream
- Likely a message ordering issue

Two possibilities:
1. Server's Init ack hasn't reached client yet when server starts sending Challenge
2. Multiple streams getting mixed up (but each has separate handler)
3. Client is sending ANOTHER Init after receiving server's ack?

## Next Steps

Need detailed CLIENT/SERVER logging to trace exact message sequence:
- When does client send Init?
- When does client receive Init ack?
- When does server send Challenge?
- What message does server actually receive instead of ChallengeResponse?

The debug logging has been added - need to rebuild and run fresh e2e tests to analyze.

