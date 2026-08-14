#!/bin/sh
# ephemeral-ttl-check.sh — TTL eviction assertion for the ephemeral presence e2e.
#
# Run AFTER node 1 has been stopped by the workflow (stop_node step). Without
# node 1's heartbeats, its presence entry in node 2's awareness store should
# expire within PRESENCE_TTL_MS (7 000 ms). Node 2's own local entry (seeded
# here) triggers the heartbeat sweep that evicts stale remotes.
#
# Args:
#   $1  node2_url   e.g. http://localhost:8931
#   $2  context_id  base58 context id
#   $3  node1_key   base58 public key of node 1's context member identity
#
# Exit 0 = TTL eviction confirmed. Exit 1 = node 1 entry still present.

set -eu

NODE2_URL="${1:-http://localhost:8931}"
CONTEXT_ID="${2:-${CONTEXT_ID:-}}"
NODE1_KEY="${3:-${NODE1_KEY:-}}"

PASS=0
FAIL=0

ok()   { echo "ok   $1 (got: ${2:-})"; PASS=$((PASS + 1)); }
fail() { echo "FAIL $1${2:+: $2}";     FAIL=$((FAIL + 1)); }

echo "=== ephemeral-ttl-check assertions ==="
echo "  node2_url   : $NODE2_URL"
echo "  context_id  : $CONTEXT_ID"
echo "  node1_key   : $NODE1_KEY"

if [ -z "$CONTEXT_ID" ] || [ -z "$NODE1_KEY" ]; then
  echo "FATAL: CONTEXT_ID or NODE1_KEY empty"
  exit 1
fi

# Seed a local slice on node 2. This ensures node 2's heartbeat_tick fires and
# sweeps expired remote entries (the sweep only runs when the node itself has
# local ephemeral state).
SET2_BODY=$(printf '{"jsonrpc":"2.0","id":3,"method":"set_ephemeral","params":{"contextId":"%s","state":[9,8,7]}}' "$CONTEXT_ID")
RC=0
SET2_RESP=$(curl -sf -m 10 -X POST "$NODE2_URL/jsonrpc" \
  -H "Content-Type: application/json" \
  -d "$SET2_BODY" 2>/dev/null) || RC=$?
# A transport failure here must NOT be swallowed: without node 2's own local
# slice the heartbeat sweep never runs, so node 1's entry would still be there
# for a reason that has nothing to do with the TTL.
if [ "$RC" -ne 0 ]; then
  echo "FATAL: set_ephemeral on node 2 failed (curl exit $RC) — the request never completed, so the sweep was never armed"
  exit 1
fi
echo "  seeded node 2 local presence to trigger sweep: $SET2_RESP"

# Wait for TTL (7 s) + 2 heartbeat ticks (2 × 2.5 s) + margin = 13 s.
# Node 1 is already stopped at this point (no more heartbeats), so the wait
# gives node 2 time to sweep node 1's stale entry.
echo "  waiting 13s for TTL (7s) + heartbeat sweeps (2×2.5s) ..."
sleep 13

GET_BODY=$(printf '{"jsonrpc":"2.0","id":4,"method":"get_ephemeral","params":{"contextId":"%s"}}' "$CONTEXT_ID")
RC=0
GET_AFTER=$(curl -sf -m 5 -X POST "$NODE2_URL/jsonrpc" \
  -H "Content-Type: application/json" \
  -d "$GET_BODY" 2>/dev/null) || RC=$?

# LOAD-BEARING: a failed request yields an empty body, and an empty body counts
# ZERO remaining entries — exactly what a genuine TTL eviction looks like. Fail
# loudly on the transport error instead of reporting a PASS that verified
# nothing.
if [ "$RC" -ne 0 ]; then
  echo "FATAL: get_ephemeral on node 2 failed (curl exit $RC) — the request never completed; the TTL guard cannot be evaluated"
  exit 1
fi

echo "  get_ephemeral response after TTL: $GET_AFTER"

# The other way this guard can pass without verifying anything: a JSON-RPC
# ERROR response (HTTP 200, so curl is happy) has no `.result`, and counting
# entries under a missing `.result` yields 0 — identical to a genuine TTL
# eviction. Require the successful shape explicitly.
HAS_RESULT=$(echo "$GET_AFTER" | jq -r 'has("result") and (.result | type == "object") and (.result | has("entries"))' 2>/dev/null || echo false)
if [ "$HAS_RESULT" != "true" ]; then
  echo "FATAL: get_ephemeral did not return result.entries — the entry count would be vacuously 0. Response: $GET_AFTER"
  exit 1
fi

# `entries` is author-keyed, so presence is a direct key lookup rather than a
# scan over a list with an `author` field. A jq failure here means the body was
# not the JSON we expect — also fatal, for the same reason.
REMAINING=$(echo "$GET_AFTER" | jq --arg k "$NODE1_KEY" \
  '[.result.entries[$k]? // empty] | length') || {
  echo "FATAL: could not parse the get_ephemeral response as JSON: $GET_AFTER"
  exit 1
}

echo "  node-1 entries remaining on node 2: $REMAINING"

if [ "$REMAINING" = "0" ]; then
  ok "node 1 entry evicted from node 2 awareness after TTL (PRESENCE_TTL_MS=7000ms)" "0 remaining"
else
  fail "node 1 entry NOT evicted from node 2 awareness after TTL" "still $REMAINING entries"
fi

echo ""
echo "=== $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ]
