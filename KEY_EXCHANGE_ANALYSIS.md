# Key Exchange Message Sequence Trace

## Expected Sequence (from code)

### Client Side (`request_key_exchange` → `authenticate_p2p`)

```
request_key_exchange:
  1. open_stream(peer)
  2. Call authenticate_p2p()

authenticate_p2p:
  3. Send Init{our_identity, our_nonce}
  4. Recv Init{their_identity, their_nonce}
  5. Call authenticate_p2p_after_init(our_nonce, their_nonce)
```

### Server Side (`handle_key_exchange`)

```
handle_key_exchange:
  1. (Init already consumed by dispatcher - has their_identity, their_nonce)
  2. Send Init{our_identity, our_nonce}
  3. Call authenticate_p2p_after_init(our_nonce, their_nonce)
```

## The Problem

**Server doesn't wait for client's Init!**

Client timeline:
```
t=0: Send Init
t=1: (waiting for server's Init ack)
t=2: Recv Init ack from server
t=3: Start authenticate_p2p_after_init
```

Server timeline:
```
t=0: Recv Init (from dispatcher)
t=1: Send Init ack
t=2: IMMEDIATELY call authenticate_p2p_after_init (doesn't wait!)
t=3: Send Challenge
t=4: Waiting for ChallengeResponse...
```

The server starts challenge-response while the client is still in Init phase!

## The Fix

The server MUST receive the client's Init ack before proceeding. But our current code doesn't do that.

Looking at master's `handle_key_share_request`, it:
1. Sends Init
2. Then calls `bidirectional_key_share` which has its OWN message sequencing

The issue is that `authenticate_p2p_after_init` ASSUMES both Inits are done. But when called from `handle_key_exchange`, the server hasn't received the client's response yet!

## Solution

`handle_key_exchange` should NOT call `authenticate_p2p_after_init`. It should:
1. Send Init ack
2. Recv client's Init (from authenticate_p2p)
3. THEN call authenticate_p2p_after_init

OR simpler: Don't send Init from `handle_key_exchange`. The Init was already sent by the dispatcher. Just call a function that expects Init to be done.

Wait, that doesn't make sense either. Let me look at what the dispatcher sends.

