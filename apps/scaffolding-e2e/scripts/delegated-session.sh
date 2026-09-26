#!/bin/sh
#
# A device that holds no node obtains a session, reads a context and submits a
# delegated write, with no password anywhere in the flow.
#
# This is the flow `delegated-authorship.yml` does not cover. That scenario proves
# a keyholder can WRITE through a relay, but it logs in with `auth_mode: embedded`
# and a username and password, which is precisely the provider the target
# architecture deletes — so its write is made by an already-privileged caller.
# What has never been exercised end to end is all three legs on one password-free
# session, which is the epic's own criterion:
#
#   A client holding only an account root and a device key can: obtain a session
#   against a node it is not the owner of, read a context it is a member of, and
#   submit a delegated write — with no password anywhere in the flow.
#
# Usage: delegated-session.sh <node> <context_id> <device_secret> <credential> <node_key> <relay_account>
#
# <node_key> is the node's own public key, captured by the scenario's
# `node_identity` step. The statement is signed over it, and the verifier checks
# it (`LoginStatement::addressed_to`): without that binding a hostile relay could
# fetch a challenge here, serve it to a user as its own, and replay what it got
# back. Passed in rather than read from an endpoint so this script asserts on the
# value the scenario already pinned.
#
# Uses curl against the node rather than meroctl, as account-api.sh does and for
# the same reason: the merod image ships no CLI, so a `target: local` script has
# none to call.

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
CONTEXT="$2"
DEVICE_SECRET="$3"
CREDENTIAL="$4"
NODE_KEY="$5"
RELAY_ACCOUNT="$6"

[ -n "${NODE}" ] || fail "usage: delegated-session.sh <node> <context> <device_secret> <credential> <node_key> <relay_account>"
[ -n "${CONTEXT}" ] || fail "no context id given"
[ -n "${DEVICE_SECRET}" ] || fail "no device secret given"
[ -n "${CREDENTIAL}" ] || fail "no device credential given"
[ -n "${NODE_KEY}" ] || fail "no node key given"
[ -n "${RELAY_ACCOUNT}" ] || fail "no relay account given — the warrant must name the node that spends it"

URL=$(node_url "${NODE}") || fail "could not resolve ${NODE}'s URL"

# --- 1-3. A session, from a device key and no password ---------------------
#
# The exchange lives in `account-api.sh` because `delegated-read-scope.sh` needs
# the same one twice over; a second copy of it would drift silently.

TOKEN=$(mint_session "${URL}" "${NODE_KEY}" "${DEVICE_SECRET}" "${CREDENTIAL}")
[ -n "${TOKEN}" ] || fail "no session was minted, and this is the criterion"
echo "session minted with no password"

# --- 4. The session reads a context it is a member of ------------------------
#
# #3931. A session authorises READS only, so this is the whole of what a
# delegated client can do without minting a warrant.

READ_RES=$(curl -sS -X POST "${URL}/admin-api/contexts/${CONTEXT}/query" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H 'Content-Type: application/json' \
    -d '{"method":"get","argsJson":{"key":"delegated"}}')
# Assert the VALUE, not merely that some field came back. The response shape
# is `QueryContextApiResponseData { returns }` (crates/server/.../query_context.rs),
# and an earlier version of this check grepped for `"output"` -- a field that
# does not exist on this route. It therefore failed while the read was
# succeeding, and would equally have passed on any body that happened to carry
# the word. `read-by-a-keyholder` is what the scenario wrote two steps up, so
# matching it proves the keyholder read THIS context's state rather than an
# empty or defaulted answer.
echo "${READ_RES}" | grep -q '"returns"[[:space:]]*:[[:space:]]*"read-by-a-keyholder"' \
    || fail "the delegated read did not return the value the scenario wrote: ${READ_RES}"
echo "delegated read served: ${READ_RES}"

# --- 5. The same session submits a delegated WRITE ---------------------------
#
# The third leg of epic #3928's criterion, and the one that had never run from a
# password-free session. `delegated-authorship.yml` already proves the warrant
# mechanics exhaustively -- refused before the grant, spendable after, replay
# refused, expiry refused -- but it logs in with `dev`/`dev-password`, so what it
# demonstrates is a delegated write by an ALREADY-PRIVILEGED caller. The criterion
# is the two halves joined: the token spent below is the one minted in step 3
# from a signed statement, and no password exists anywhere in this file.

# The EXACT bytes the warrant commits to and the intent sends. One variable for
# both, because the warrant carries `H(method ‖ args)`: re-typing this JSON in
# the request with different spacing mints a warrant that verifies as a
# signature and is refused as authorisation -- a failure that reads as a broken
# gate rather than as a mismatched hash.
INTENT_ARGS='{"key":"delegated","value":"written-by-a-keyholder"}'

# --- 5a. The device mints a warrant, offline --------------------------------
#
# Same device secret and credential as the login statement, and `merod account
# warrant` for the same reason `login-statement` is used above: the borsh layout
# and signing domain live in one place, and a harness carrying its own copy
# passes its own checks while every real client is refused.
#
# `--executor` is node-1's own account -- the node this session is talking to is
# the node that spends the warrant. The scenario granted it
# `CAN_AUTHOR_ON_BEHALF` one step up; without that the POST below is refused,
# which is exactly what `delegated-authorship.yml` asserts separately.
WARRANT=$(offline_merod account warrant \
    --context "${CONTEXT}" \
    --executor "${RELAY_ACCOUNT}" \
    --method set \
    --args "${INTENT_ARGS}" \
    --nonce 1 \
    --valid-for 300 \
    --device-secret "${DEVICE_SECRET}" \
    --credential "${CREDENTIAL}" | tr -d '\r' | grep -E '^[0-9a-f]{200,}$' | head -1)
[ -n "${WARRANT}" ] || fail "merod minted no warrant"
echo "warrant minted offline"

# --- 5b. The keyholder spends it, on its own session ------------------------
#
# `Authorization: Bearer ${TOKEN}` is the load-bearing part. The provider's
# `session_permissions` include `context:intent`, so this is what a delegated
# session is FOR -- and a scenario that posted the intent with node-1's admin
# credentials would prove nothing about the session at all.
INTENT_BODY=$(printf '{"method":"set","argsJson":%s,"warrant":"%s","authorProof":"%s"}' \
    "${INTENT_ARGS}" "${WARRANT}" "${CREDENTIAL}")

INTENT_RES=$(curl -sS -X POST "${URL}/admin-api/contexts/${CONTEXT}/intents" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H 'Content-Type: application/json' \
    -d "${INTENT_BODY}")
# `rootHash` is only present when the intent actually executed and advanced
# state. Asserting on it rather than on the absence of an error means a refusal
# body, an empty body, and a 500 all fail here rather than one of them slipping
# through as success.
echo "${INTENT_RES}" | grep -q '"rootHash"' \
    || fail "the delegated write was not accepted on a password-free session: ${INTENT_RES}"
echo "delegated write accepted: ${INTENT_RES}"

# --- 5c. The write is visible to the same session ---------------------------
#
# Read back through the delegated READ path, so the value is observed the way a
# client would observe it rather than through an admin surface the keyholder
# does not have.
AFTER=$(curl -sS -X POST "${URL}/admin-api/contexts/${CONTEXT}/query" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H 'Content-Type: application/json' \
    -d '{"method":"get","argsJson":{"key":"delegated"}}')
echo "${AFTER}" | grep -q '"returns"[[:space:]]*:[[:space:]]*"written-by-a-keyholder"' \
    || fail "the delegated write did not change what the session reads: ${AFTER}"
echo "delegated write is visible to its author's session"

# --- 5d. The spent warrant cannot be spent again ----------------------------
#
# The security property that makes the whole path safe to expose: the same bytes
# that just succeeded must now fail. Without this a relay could replay a
# member's warrant at a time of its choosing, for as long as it stayed valid.
# `delegated-authorship.yml` asserts this too; it is repeated here because this
# is the session-authenticated path, and the nonce window is checked against the
# CONTEXT rather than against whatever authenticated the caller.
REPLAY_RES=$(curl -sS -X POST "${URL}/admin-api/contexts/${CONTEXT}/intents" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H 'Content-Type: application/json' \
    -d "${INTENT_BODY}")
if echo "${REPLAY_RES}" | grep -q '"rootHash"'; then
    fail "the spent warrant was accepted a second time — the nonce was not consumed: ${REPLAY_RES}"
fi
echo "replay refused: ${REPLAY_RES}"

# --- What this does NOT assert, stated rather than implied -------------------
#
# That the delta is ATTRIBUTED to the author rather than to the relay. That is
# the point of `Principal`, and it is real, but it is not observable from the
# two HTTP responses above -- the author lives on the delta, not in the intent
# reply. The scenario's peer read-back proves the write propagated and was
# accepted by a node that re-verifies both signature layers, which is the part
# reachable from here; attribution itself is covered by the unit tests around
# `Principal` and by `delegated-authorship.yml`'s DAG assertions.

echo "PASS: a device holding only a key obtained a session, read a context, and submitted a delegated write — with no password in the flow"
