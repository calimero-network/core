#!/bin/sh
# ephemeral-presence-e2e.sh — assertion script for the ephemeral presence e2e.
#
# Called by merobox as a `script` step (target: local). All merobox dynamic
# values are available as uppercase env vars; the ones this script uses:
#
#   CONTEXT_ID          – base58 context id (from create_context output)
#   NODE1_KEY           – base58 public key of node 1's context member identity
#   NODE1_URL           – http://localhost:<rpc_port> for node 1
#   NODE2_URL           – http://localhost:<rpc_port> for node 2
#
# Exit 0 = all assertions passed. Exit 1 = at least one assertion failed.
#
# Auth: nodes are in Proxy mode (default); no Bearer token required.
# The /jsonrpc and /admin-api endpoints are reachable without auth in this mode.

set -eu

NODE1_URL="${1:-http://localhost:8930}"
NODE2_URL="${2:-http://localhost:8931}"
CONTEXT_ID="${3:-${CONTEXT_ID:-}}"
NODE1_KEY="${4:-${NODE1_KEY:-}}"

PASS=0
FAIL=0

# --- helpers -----------------------------------------------------------------

ok() {
  label="$1"
  echo "ok   $label"
  PASS=$((PASS + 1))
}

fail() {
  label="$1"; detail="${2:-}"
  echo "FAIL $label${detail:+: $detail}"
  FAIL=$((FAIL + 1))
}

check() {
  # check <label> <expected> <actual>
  label="$1"; expected="$2"; actual="$3"
  if [ "$actual" = "$expected" ]; then
    ok "$label (got: $actual)"
  else
    fail "$label" "expected=$expected actual=$actual"
  fi
}

# A curl that never completed is NOT a failed assertion — it is an unusable
# run. The response body is empty either way, and an empty body silently
# satisfies several of the checks below (empty `.error`, zero entries), so a
# masked transport failure reads as a PASS. Report it and stop.
die_curl() {
  label="$1"; rc="$2"
  fail "$label" "curl exit $rc — the request never completed; downstream assertions would be meaningless"
  echo ""
  echo "=== $PASS passed, $FAIL failed ==="
  exit 1
}

check_not_empty() {
  label="$1"; val="$2"
  if [ -n "$val" ] && [ "$val" != "null" ] && [ "$val" != "none" ]; then
    ok "$label (got: $val)"
  else
    fail "$label" "value is empty or null"
  fi
}

# --- sanity ------------------------------------------------------------------

echo "=== ephemeral-presence-e2e assertions ==="
echo "  node1_url   : $NODE1_URL"
echo "  node2_url   : $NODE2_URL"
echo "  context_id  : $CONTEXT_ID"
echo "  node1_key   : $NODE1_KEY"

if [ -z "$CONTEXT_ID" ]; then
  echo "FATAL: CONTEXT_ID is empty — was the create_context step output captured?"
  exit 1
fi

# --- Phase 0: verify both nodes have the context ----------------------------

echo ""
echo "-- Phase 0: context reachable on both nodes --"

RC=0
CTX1=$(curl -sf -m 10 "$NODE1_URL/admin-api/contexts/$CONTEXT_ID" 2>/dev/null) || RC=$?
[ "$RC" -eq 0 ] || die_curl "node1 context API responds" "$RC"

RC=0
CTX2=$(curl -sf -m 10 "$NODE2_URL/admin-api/contexts/$CONTEXT_ID" 2>/dev/null) || RC=$?
[ "$RC" -eq 0 ] || die_curl "node2 context API responds" "$RC"

check_not_empty "node1 context API responds" "$(echo "$CTX1" | jq -r '.data.id // empty' 2>/dev/null || true)"
check_not_empty "node2 context API responds" "$(echo "$CTX2" | jq -r '.data.id // empty' 2>/dev/null || true)"

# Log context state hashes for diagnostic purposes.
echo "  node1 contextStateHash : $(echo "$CTX1" | jq -r '.data.contextStateHash // "null"' 2>/dev/null || true)"
echo "  node2 contextStateHash : $(echo "$CTX2" | jq -r '.data.contextStateHash // "null"' 2>/dev/null || true)"

# --- Phase 1: advance node 1's DAG with a real kv-store write ----------------
#
# The no-DAG-growth guard (Phase 4) is only meaningful if the DAG has something
# to protect. We drive a genuine persisted write on node 1 (which has kv_store
# installed and created the context, so this needs NO cross-node sync), then
# assert node 1's contextStateHash is NON-NULL — proving the DAG actually
# advanced. Later we re-check it is UNCHANGED after set_ephemeral, so the guard
# is directly falsifiable: any DAG op emitted by the ephemeral handler would
# move this hash and fail the test.
#
# The genesis / null hash is base58 of [0u8; 32] == "11111111111111111111111111111111".
NULL_HASH="11111111111111111111111111111111"

echo ""
echo "-- Phase 1: advance node 1 DAG with a real kv-store write, capture NON-NULL hash --"

# execute wire shape (see calimero_server_primitives::jsonrpc::ExecutionRequest):
#   params = { contextId, method, argsJson }  — executor identity is auto-
#   resolved server-side to the node's owned key (no executorPublicKey field).
EXEC_BODY=$(printf '{"jsonrpc":"2.0","id":10,"method":"execute","params":{"contextId":"%s","method":"set","argsJson":{"key":"dag-guard","value":"1"}}}' "$CONTEXT_ID")

RC=0
EXEC_RESP=$(curl -sf -m 15 -X POST "$NODE1_URL/jsonrpc" \
  -H "Content-Type: application/json" \
  -d "$EXEC_BODY" 2>/dev/null) || RC=$?
# Without this, an empty body yields an empty `.error` and the check below
# reports "execute(set) succeeded" for a request that never reached the node.
[ "$RC" -eq 0 ] || die_curl "kv-store execute(set) on node 1" "$RC"

echo "  execute(set) response: $EXEC_RESP"

EXEC_ERROR=$(echo "$EXEC_RESP" | jq -r '.error // empty' 2>/dev/null || true)
if [ -n "$EXEC_ERROR" ] && [ "$EXEC_ERROR" != "null" ]; then
  fail "kv-store execute(set) on node 1" "RPC error: $EXEC_ERROR"
else
  ok "kv-store execute(set) on node 1 succeeded (no RPC error)"
fi

# Capture node 1's hash AFTER the write. This is the baseline the guard protects.
RC=0
CTX1_AFTER_WRITE=$(curl -sf -m 10 "$NODE1_URL/admin-api/contexts/$CONTEXT_ID" 2>/dev/null) || RC=$?
[ "$RC" -eq 0 ] || die_curl "read node1 contextStateHash after kv write" "$RC"
HASH_BEFORE=$(echo "$CTX1_AFTER_WRITE" | jq -r '.data.contextStateHash // empty' 2>/dev/null || true)

echo "  node1 contextStateHash after kv write: $HASH_BEFORE"

# Assert the DAG actually advanced (hash is not the genesis/null hash).
if [ -n "$HASH_BEFORE" ] && [ "$HASH_BEFORE" != "$NULL_HASH" ]; then
  ok "node1 contextStateHash is NON-NULL after kv write — DAG advanced (got: $HASH_BEFORE)"
else
  fail "node1 contextStateHash is NON-NULL after kv write" "still genesis/null: $HASH_BEFORE — guard would be vacuous"
fi

# --- Phase 2: call set_ephemeral on node 1 -----------------------------------

echo ""
echo "-- Phase 2: set_ephemeral on node 1 --"

# state = [1, 2, 3] — an arbitrary small presence slice
SET_BODY=$(printf '{"jsonrpc":"2.0","id":1,"method":"set_ephemeral","params":{"contextId":"%s","state":[1,2,3]}}' "$CONTEXT_ID")

RC=0
SET_RESP=$(curl -sf -m 10 -X POST "$NODE1_URL/jsonrpc" \
  -H "Content-Type: application/json" \
  -d "$SET_BODY" 2>/dev/null) || RC=$?
# Same masking as Phase 1: an empty body means an empty `.error`, which the
# check below would read as "no RPC error".
[ "$RC" -eq 0 ] || die_curl "set_ephemeral on node 1" "$RC"

echo "  set_ephemeral response: $SET_RESP"

SET_ERROR=$(echo "$SET_RESP" | jq -r '.error // empty' 2>/dev/null || true)
if [ -n "$SET_ERROR" ] && [ "$SET_ERROR" != "null" ]; then
  fail "set_ephemeral on node 1" "RPC error: $SET_ERROR"
else
  SET_RESULT=$(echo "$SET_RESP" | jq -r '.result // empty' 2>/dev/null || true)
  check_not_empty "set_ephemeral returned result" "$SET_RESULT"
  ok "set_ephemeral: no RPC error"
fi

# --- Phase 3: poll get_ephemeral on node 2 for node 1's entry ---------------

echo ""
echo "-- Phase 3: poll node 2 get_ephemeral until node 1 entry appears --"

GET_BODY=$(printf '{"jsonrpc":"2.0","id":2,"method":"get_ephemeral","params":{"contextId":"%s"}}' "$CONTEXT_ID")

FOUND=0
ATTEMPTS=0
CURL_FAILS=0
LAST_RC=0
MAX_ATTEMPTS=30   # 30 × 0.5s = 15s budget

while [ $ATTEMPTS -lt $MAX_ATTEMPTS ]; do
  sleep 0.5
  ATTEMPTS=$((ATTEMPTS + 1))

  RC=0
  GET_RESP=$(curl -sf -m 5 -X POST "$NODE2_URL/jsonrpc" \
    -H "Content-Type: application/json" \
    -d "$GET_BODY" 2>/dev/null) || RC=$?

  # A single failed poll is retriable (the node may still be coming up), but
  # it must not masquerade as "polled successfully, no entries yet" — count it
  # so the failure message below can say which of the two happened.
  if [ "$RC" -ne 0 ]; then
    CURL_FAILS=$((CURL_FAILS + 1))
    LAST_RC=$RC
    GET_RESP=""
    continue
  fi

  # `entries` is an OBJECT keyed by author (base58), not a list.
  ENTRIES=$(echo "$GET_RESP" | jq '.result.entries // {}' 2>/dev/null || true)
  COUNT=$(echo "$ENTRIES" | jq 'length' 2>/dev/null || echo 0)

  if [ "$COUNT" -gt 0 ]; then
    echo "  got $COUNT entries from node 2 after $ATTEMPTS polls (${ATTEMPTS}×0.5s)"
    FOUND=1
    break
  fi
done

if [ "$FOUND" = "1" ]; then
  ok "get_ephemeral on node 2 received at least 1 entry within 15s"
elif [ "$CURL_FAILS" -eq "$ATTEMPTS" ]; then
  # Every poll failed at the transport — this says nothing about gossip.
  die_curl "get_ephemeral on node 2 (all $ATTEMPTS polls failed)" "$LAST_RC"
else
  fail "get_ephemeral on node 2 received 0 entries after 15s — gossip not delivered" \
    "($CURL_FAILS of $ATTEMPTS polls also failed at the transport)"
  # Still proceed so subsequent assertions capture all failures
  GET_RESP=""
  ENTRIES="{}"
fi

# Verify the state bytes of the first entry are [1,2,3].
# Entries are author-keyed, so index by value rather than position.
# Use -c (compact) so the comparison is a single-line string.
ENTRY_STATE=$(echo "$ENTRIES" | jq -c 'to_entries[0].value.state // empty' 2>/dev/null || true)
echo "  entry state (first): $ENTRY_STATE"
check "node 2 entry state equals [1,2,3]" "[1,2,3]" "$ENTRY_STATE"

# If node1_key was provided, verify the author field
if [ -n "$NODE1_KEY" ]; then
  # The author is now the map KEY, not a field on the value.
  ENTRY_AUTHOR=$(echo "$ENTRIES" | jq -r 'to_entries[0].key // empty' 2>/dev/null || true)
  echo "  entry author (first): $ENTRY_AUTHOR"
  check "node 2 entry author matches node 1 key" "$NODE1_KEY" "$ENTRY_AUTHOR"

  # The entry must also be keyed directly by node 1's public key — this is what
  # a client does (index by author) rather than scanning a list.
  KEYED_STATE=$(echo "$ENTRIES" | jq -c --arg k "$NODE1_KEY" '.[$k].state // empty' 2>/dev/null || true)
  check "node 2 entry is directly indexable by author key" "[1,2,3]" "$KEYED_STATE"

  # Age must be present and within the TTL window (7000 ms). A live author that
  # just published should be far fresher than that.
  ENTRY_AGE=$(echo "$ENTRIES" | jq -r --arg k "$NODE1_KEY" '.[$k].ageMs // empty' 2>/dev/null || true)
  echo "  entry ageMs: $ENTRY_AGE"
  if [ -n "$ENTRY_AGE" ] && [ "$ENTRY_AGE" -ge 0 ] && [ "$ENTRY_AGE" -lt 7000 ]; then
    ok "node 2 entry carries an ageMs inside the TTL window (got: $ENTRY_AGE)"
  else
    fail "node 2 entry ageMs missing or outside the TTL window" "got: '$ENTRY_AGE'"
  fi
else
  echo "  (node1_key not provided — skipping author assertion)"
fi

# --- Phase 4: no-DAG-growth guard — node 1 hash UNCHANGED by set_ephemeral ---
#
# LOAD-BEARING. We re-read node 1's contextStateHash (the same node the
# ephemeral write hit) and assert it equals the NON-NULL baseline captured in
# Phase 1. Because the baseline is a real, DAG-advanced hash, this is directly
# falsifiable: if set_ephemeral had emitted any DAG op, node 1's hash would
# have moved and this assertion would fail.

echo ""
echo "-- Phase 4: no-DAG-growth guard (node 1, falsifiable) --"

RC=0
CTX1_AFTER_EPHEMERAL=$(curl -sf -m 10 "$NODE1_URL/admin-api/contexts/$CONTEXT_ID" 2>/dev/null) || RC=$?
# An empty body here would compare unequal to the baseline and fail — but for
# the wrong reason, and on the load-bearing guard. Say what actually happened.
[ "$RC" -eq 0 ] || die_curl "read node1 contextStateHash after set_ephemeral" "$RC"
HASH_AFTER=$(echo "$CTX1_AFTER_EPHEMERAL" | jq -r '.data.contextStateHash // empty' 2>/dev/null || true)

echo "  node1 hash before set_ephemeral (NON-NULL baseline): $HASH_BEFORE"
echo "  node1 hash after  set_ephemeral                    : $HASH_AFTER"

check "no DAG growth: node1 contextStateHash unchanged by set_ephemeral (LOAD-BEARING)" \
  "$HASH_BEFORE" "$HASH_AFTER"

# --- Summary -----------------------------------------------------------------
# Phase 5 (TTL expiry) is a separate script step in the workflow; it runs
# AFTER the workflow has stopped node 1, so that no more heartbeats from
# node 1 can refresh its entry on node 2. See ephemeral-ttl-check.sh.

echo ""
echo "=== $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ]
