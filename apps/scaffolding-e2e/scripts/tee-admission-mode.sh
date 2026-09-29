#!/bin/sh
#
# The TEE admission mode, seen from the only route it changes: `/intents`.
#
# Two mock TEEs, each fleet-joined to its own namespace, identical but for that
# namespace's admission policy: one says `mode: relay` and admits a `RelayTee`,
# the other leaves `mode` out and admits a `ReadOnlyTee` (the default). Every
# check below runs against BOTH, and the pair is what gives each one meaning: a
# 200 from the relay alone proves only that something answered, and a 403 from
# the replica alone could be any refusal. The same request shape answering 200
# on one and the named replica refusal on the other proves the mode decided it.
#
# Usage: tee-admission-mode.sh <relay_node> <replica_node> \
#          <relay_context> <replica_context> <relay_account> <replica_account> \
#          <relay_warrant> <replica_warrant> <credential>
#
# curl, not a client library, because the refusal's exact status and body are
# the assertion, and a client turns both into an exception string.

set -e

. "$(dirname "$0")/account-api.sh"

RELAY_NODE="$1"
REPLICA_NODE="$2"
RELAY_CONTEXT="$3"
REPLICA_CONTEXT="$4"
RELAY_ACCOUNT="$5"
REPLICA_ACCOUNT="$6"
RELAY_WARRANT="$7"
REPLICA_WARRANT="$8"
CREDENTIAL="$9"

USAGE="usage: tee-admission-mode.sh <relay_node> <replica_node> <relay_ctx> <replica_ctx> <relay_account> <replica_account> <relay_warrant> <replica_warrant> <credential>"
for _arg in "${RELAY_NODE}" "${REPLICA_NODE}" "${RELAY_CONTEXT}" "${REPLICA_CONTEXT}" \
    "${RELAY_ACCOUNT}" "${REPLICA_ACCOUNT}" "${RELAY_WARRANT}" "${REPLICA_WARRANT}" \
    "${CREDENTIAL}"; do
    [ -n "${_arg}" ] || fail "${USAGE}"
done

RELAY_URL=$(node_url "${RELAY_NODE}") || fail "could not resolve ${RELAY_NODE}'s URL"
REPLICA_URL=$(node_url "${REPLICA_NODE}") || fail "could not resolve ${REPLICA_NODE}'s URL"

# The refusal core#4180 names a replica with. Matched whole: a looser pattern
# would also pass on the generic "no CAN_AUTHOR_ON_BEHALF grant" refusal, which
# is the answer this mode split exists to stop giving a TEE.
REPLICA_REFUSAL='this node is a TEE replica (ReadOnlyTee) and does not relay writes; the namespace must admit relays with mode=relay'

PASS=0
FAIL=0

check() { # check <label> <expected> <actual>
    if [ "$3" = "$2" ]; then
        echo "ok   $1 ($3)"
        PASS=$((PASS + 1))
    else
        echo "FAIL $1: expected $2, got $3"
        FAIL=$((FAIL + 1))
    fi
}

BODY_FILE=$(mktemp)
trap 'rm -f "${BODY_FILE}"' EXIT

# request <method> <url> [body] -> prints the status; the body lands in BODY_FILE.
request() {
    _m="$1" _u="$2" _b="${3:-}"
    if [ -n "${_b}" ]; then
        curl -s -o "${BODY_FILE}" -w '%{http_code}' -m 30 -X "${_m}" \
            -H 'Content-Type: application/json' -d "${_b}" "${_u}"
    else
        curl -s -o "${BODY_FILE}" -w '%{http_code}' -m 30 -X "${_m}" "${_u}"
    fi
}

# json_field <name> -> the first scalar value of that key, at any depth, in BODY_FILE.
json_field() {
    sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\{0,1\}\([^\",}]*\)\"\{0,1\}.*/\1/p" "${BODY_FILE}" | head -1
}

echo "== tee-admission-mode: ${RELAY_NODE} admitted with mode=relay, ${REPLICA_NODE} with the default =="

# --- 1. Discovery: what a client reads before it mints -----------------------
#
# The same gate the POST asks, answered before a nonce is spent. The executor
# account is also what both warrants were minted for, so a mismatch here would
# make the POSTs below fail for a reason unrelated to the mode.

check "GET intents on the relay" 200 \
    "$(request GET "${RELAY_URL}/admin-api/contexts/${RELAY_CONTEXT}/intents")"
check "the relay's discovery names its account" "${RELAY_ACCOUNT}" "$(json_field executorAccount)"
check "the relay may author on a member's behalf" true "$(json_field canAuthorOnBehalf)"

check "GET intents on the replica" 200 \
    "$(request GET "${REPLICA_URL}/admin-api/contexts/${REPLICA_CONTEXT}/intents")"
check "the replica's discovery names its account" "${REPLICA_ACCOUNT}" "$(json_field executorAccount)"
check "the replica may not author on a member's behalf" false "$(json_field canAuthorOnBehalf)"

# --- 2. The relayed write ----------------------------------------------------
#
# Arguments byte-identical to the `--args` each warrant was minted over: the
# warrant commits to a hash of them.

RELAY_BODY=$(printf '{"method":"set","argsJson":{"key":"relayed","value":"through-a-relay-tee"},"warrant":"%s","authorProof":"%s"}' \
    "${RELAY_WARRANT}" "${CREDENTIAL}")
REPLICA_BODY=$(printf '{"method":"set","argsJson":{"key":"relayed","value":"through-a-replica-tee"},"warrant":"%s","authorProof":"%s"}' \
    "${REPLICA_WARRANT}" "${CREDENTIAL}")

check "POST intent through the RelayTee" 200 \
    "$(request POST "${RELAY_URL}/admin-api/contexts/${RELAY_CONTEXT}/intents" "${RELAY_BODY}")"
cat "${BODY_FILE}"; echo
check "the relayed intent reports a root hash" set \
    "$([ -n "$(json_field rootHash)" ] && echo set || echo unset)"

check "POST intent through the ReadOnlyTee" 403 \
    "$(request POST "${REPLICA_URL}/admin-api/contexts/${REPLICA_CONTEXT}/intents" "${REPLICA_BODY}")"
cat "${BODY_FILE}"; echo
check "the replica is refused by its role, by name" named \
    "$(grep -qF "${REPLICA_REFUSAL}" "${BODY_FILE}" && echo named || echo other)"

echo "== ${PASS} passed, ${FAIL} failed =="
[ "${FAIL}" -eq 0 ] || fail "the TEE admission mode does not decide who relays"
