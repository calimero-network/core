#!/bin/sh
#
# A caller with no session, no token and no account on the node it is talking to
# reads a context it is a member of — by signing the request.
#
# `delegated-session.yml` proves the SESSION path: log in with a device key, get
# a token, present the token. This is the other half of the architecture and
# nothing had ever exercised it end to end. `RequestSig`, `CallerProof` and the
# guard that verifies them are covered by unit tests and by fixtures three
# implementations agree on — but no running node had ever accepted a signed
# request, which is exactly the class a green `cargo test` cannot speak to.
#
# Deliberately NO auth provider is enabled on any node here. The proof path is
# gated by `--delegated-access` alone, and a scenario that turned on
# `--device-key-login` as well would not notice if that stopped being true.
#
# Usage: delegated-proof.sh <relay> <member_node> <unflagged> <context> <credential> <device_secret> <relay_node_key> <unflagged_node_key>

set -e

. "$(dirname "$0")/account-api.sh"

RELAY="$1"
MEMBER_NODE="$2"
UNFLAGGED="$3"
CONTEXT="$4"
CREDENTIAL="$5"
DEVICE_SECRET="$6"
RELAY_KEY="$7"
UNFLAGGED_KEY="$8"

for v in RELAY MEMBER_NODE UNFLAGGED CONTEXT CREDENTIAL DEVICE_SECRET RELAY_KEY UNFLAGGED_KEY; do
    eval "val=\${$v}"
    [ -n "${val}" ] || fail "usage: delegated-proof.sh <relay> <member_node> <unflagged> <context> <credential> <device_secret> <relay_node_key> <unflagged_node_key>"
done

RELAY_URL=$(node_url "${RELAY}") || fail "could not resolve ${RELAY}"
MEMBER_URL=$(node_url "${MEMBER_NODE}") || fail "could not resolve ${MEMBER_NODE}"
UNFLAGGED_URL=$(node_url "${UNFLAGGED}") || fail "could not resolve ${UNFLAGGED}"

# --- 1. A session key, certified by the device, addressed to the relay -------
#
# The three-link chain, not the two-link one. Only the session link carries a
# node, so it is the only shape with a node binding at all — which is the
# property asserted below. `CallerProof::verify` says so in its own docs: "on
# the short chain there is nothing to bind".
#
# The challenge is synthetic on purpose. A challenge is spent at `/auth/token`,
# and this flow never goes near it — so requiring a real one would couple the
# proof path to a login provider it does not use.
CHALLENGE="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"

# A statement is addressed to ONE node, so a second node needs its own. Both
# come from the same device, which is the point: the account is identical and
# only the node it names differs.
mint_statement() {
    _for_node="$1"
    offline_merod account login-statement \
        --challenge "${CHALLENGE}" \
        --node "${_for_node}" \
        --device-secret "${DEVICE_SECRET}" \
        --generate-session-key \
        --credential "${CREDENTIAL}" \
        --audience cli
}

SIGNED=$(mint_statement "${RELAY_KEY}")
STATEMENT=$(echo "${SIGNED}" | head -1)
SESSION_SECRET=$(echo "${SIGNED}" | sed -n 's/^Session-Secret:[[:space:]]*//p')
[ -n "${STATEMENT}" ] || fail "merod produced no login statement"
[ -n "${SESSION_SECRET}" ] || fail "merod produced no session secret"

# --- 2. Sign one request, and take the whole header -------------------------
#
# `--credential` makes `sign-request` print the assembled `X-Calimero-Proof`
# rather than the bare signature. Assembled by merod rather than here, so this
# script carries no copy of an encoding the node has to agree with.
sign_proof_with() {
    _statement="$1"
    _secret="$2"
    _method="$3"
    _path="$4"
    offline_merod account sign-request \
        --method "${_method}" \
        --path "${_path}" \
        --signer-secret "${_secret}" \
        --credential "${CREDENTIAL}" \
        --session "${_statement}" \
        --valid-for 300 \
        | sed -n 's/^Proof:[[:space:]]*//p'
}

sign_proof() {
    sign_proof_with "${STATEMENT}" "${SESSION_SECRET}" "$1" "$2"
}

PROOF=$(sign_proof GET /admin-api/contexts)
[ -n "${PROOF}" ] || fail "merod printed no proof header"
echo "signed a request; proof is ${#PROOF} hex chars"

# GET a path with a proof header, leaving the status in CODE and body in BODY.
probe() {
    _url="$1"
    _path="$2"
    _proof="$3"
    _out=$(mktemp)
    if [ -n "${_proof}" ]; then
        CODE=$(curl -sS -o "${_out}" -w '%{http_code}' \
            -H "X-Calimero-Proof: ${_proof}" "${_url}${_path}")
    else
        CODE=$(curl -sS -o "${_out}" -w '%{http_code}' "${_url}${_path}")
    fi
    BODY=$(cat "${_out}")
    rm -f "${_out}"
}

expect() {
    _what="$1"
    _want="$2"
    [ "${CODE}" = "${_want}" ] || fail "${_what}: expected ${_want}, got ${CODE} -- ${BODY}"
    echo "  ok ${_what} -> ${CODE}"
}

FAILURES=0

# --- 3. The criterion -------------------------------------------------------
echo "== the relay serves a signed request =="
probe "${RELAY_URL}" /admin-api/contexts "${PROOF}"
expect "a signed request is served by the relay" 200
# The VALUE, not just the status: a 200 with an empty list would mean the proof
# authenticated and the scoping then hid everything, which is a different bug
# and passes a status-only check.
echo "${BODY}" | grep -q "${CONTEXT}" \
    || fail "the relay served the request but listed no context: ${BODY}"
echo "  ok the listing contains the caller's own context"

# --- 4. The node binding ----------------------------------------------------
echo "== the same proof is refused elsewhere =="
# Minted for the relay's key. The member's own node also serves proofs, so a
# refusal here can only be the node binding — which is the whole reason the
# session link carries a node.
probe "${MEMBER_URL}" /admin-api/contexts "${PROOF}"
expect "a proof minted for another node is refused" 401

# --- 5. The flag actually gates it ------------------------------------------
echo "== a node not serving delegated access refuses =="
# Addressed to NODE-3, deliberately. `admit` verifies the chain before it
# consults the policy, so the proof above — minted for the relay — would fail
# the node binding here and never reach the branch this is about. The refusal
# would still be 401, and would say nothing about the flag.
UNFLAGGED_SIGNED=$(mint_statement "${UNFLAGGED_KEY}")
UNFLAGGED_STATEMENT=$(echo "${UNFLAGGED_SIGNED}" | head -1)
UNFLAGGED_SECRET=$(echo "${UNFLAGGED_SIGNED}" | sed -n 's/^Session-Secret:[[:space:]]*//p')
[ -n "${UNFLAGGED_STATEMENT}" ] || fail "merod minted no statement for the unflagged node"

# 403, not 401: this node CAN verify the chain, and the answer is "I was not
# asked to serve that account" — a different statement from "your credential is
# bad", and the one that tells a caller to stop retrying.
probe "${UNFLAGGED_URL}" /admin-api/contexts \
    "$(sign_proof_with "${UNFLAGGED_STATEMENT}" "${UNFLAGGED_SECRET}" GET /admin-api/contexts)"
expect "a node without --delegated-access refuses a chain it can verify" 403

# --- 6. The signature is bound to the request -------------------------------
echo "== a proof does not travel between requests =="
OTHER=$(sign_proof GET /admin-api/namespaces)
probe "${RELAY_URL}" /admin-api/contexts "${OTHER}"
expect "a proof minted for another path is refused" 401

probe "${RELAY_URL}" /admin-api/contexts "$(sign_proof POST /admin-api/contexts)"
expect "a proof minted for another method is refused" 401

# --- 7. Opening this did not open the route ---------------------------------
echo "== no proof is still no entry =="
probe "${RELAY_URL}" /admin-api/contexts ""
expect "an unauthenticated caller is still refused" 401

probe "${RELAY_URL}" /admin-api/contexts "not-a-proof"
expect "a malformed proof is refused" 401

[ "${FAILURES}" -eq 0 ] || fail "${FAILURES} assertion(s) failed"
echo "delegated proof holds: signed requests are served, and only where and for what they were signed"
