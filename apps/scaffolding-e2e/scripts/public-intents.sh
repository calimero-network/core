#!/bin/sh
#
# The relay posture: delegated execution served with no node credential at all.
#
# This is what a fleet node actually runs, and until this scenario nothing
# exercised it. `--public-intents` appeared nowhere in apps/, .github/,
# e2e-tests/ or workflows/ — the flag that decides whether a hosted relay is
# reachable had no test, so the only evidence it worked was that production
# had not visibly broken.
#
# Two nodes, identical but for the flag. Every assertion below is made against
# BOTH, and the pair is what makes them mean something: a single node answering
# 200 proves only that something answered. The same request answering 200 on the
# node with the flag and 401 on the node without it proves the flag is what
# opened the route.
#
# Usage: public-intents.sh <open_node> <closed_node> <context_id> <warrant> <credential>
#
# Uses curl rather than meroctl for the same reason account-api.sh does: the
# merod image ships no CLI, so a `target: local` script has none to call. It also
# has to send requests with NO Authorization header, which is precisely what a
# client library will not do for you.

set -e

. "$(dirname "$0")/account-api.sh"

OPEN_NODE="$1"
CLOSED_NODE="$2"
CONTEXT="$3"
WARRANT="$4"
CREDENTIAL="$5"

[ -n "${OPEN_NODE}" ] || fail "usage: public-intents.sh <open_node> <closed_node> <context> <warrant> <credential>"
[ -n "${CLOSED_NODE}" ] || fail "no closed node given — the control is not optional here"
[ -n "${CONTEXT}" ] || fail "no context id given"
[ -n "${WARRANT}" ] || fail "no warrant given"
[ -n "${CREDENTIAL}" ] || fail "no author credential given"

OPEN_URL=$(node_url "${OPEN_NODE}") || fail "could not resolve ${OPEN_NODE}'s URL"
CLOSED_URL=$(node_url "${CLOSED_NODE}") || fail "could not resolve ${CLOSED_NODE}'s URL"

PASS=0
FAIL=0

# Deliberately no Authorization header anywhere below. A helper that quietly
# added one would make every assertion here pass for the wrong reason.
status_of() { # status_of <method> <url> [body]
    _m="$1" _u="$2" _b="${3:-}"
    if [ -n "${_b}" ]; then
        curl -s -o /dev/null -w '%{http_code}' -m 15 -X "${_m}" \
            -H 'Content-Type: application/json' -d "${_b}" "${_u}"
    else
        curl -s -o /dev/null -w '%{http_code}' -m 15 -X "${_m}" "${_u}"
    fi
}

check() { # check <label> <expected> <actual>
    if [ "$3" = "$2" ]; then
        echo "ok   $1 ($3)"
        PASS=$((PASS + 1))
    else
        echo "FAIL $1: expected $2, got $3"
        FAIL=$((FAIL + 1))
    fi
}

echo "== public-intents: ${OPEN_NODE} has the flag, ${CLOSED_NODE} does not =="

# --- 1. Discovery, unauthenticated -------------------------------------------
#
# The half a client needs before it can mint anything: which account to name as
# the warrant's executor. Refused here, a client cannot even address a warrant
# correctly, and learns that only after spending a nonce on one.

check "GET intents on the open node" 200 \
    "$(status_of GET "${OPEN_URL}/admin-api/contexts/${CONTEXT}/intents")"

check "GET intents on the closed node" 401 \
    "$(status_of GET "${CLOSED_URL}/admin-api/contexts/${CONTEXT}/intents")"

# The body has to carry the executor account, not merely exist. A 200 with an
# empty payload would satisfy the status check above and be useless to a client.
RELAY_ACCOUNT=$(curl -s -m 15 "${OPEN_URL}/admin-api/contexts/${CONTEXT}/intents" \
    | sed -n 's/.*"executorAccount"[[:space:]]*:[[:space:]]*"\([0-9a-f]*\)".*/\1/p')
[ -n "${RELAY_ACCOUNT}" ] || fail "the open node's discovery answer names no executor account"
echo "ok   discovery names the executor account (${RELAY_ACCOUNT})"
PASS=$((PASS + 1))

# --- 2. The write itself, unauthenticated ------------------------------------
#
# The warrant is the credential. The arguments must be byte-identical to the
# ones the warrant was minted over -- it commits to a hash of them, so
# re-indenting this JSON mints a request that verifies as a signature and is
# refused as authorisation.

INTENT_BODY=$(printf '{"method":"set","argsJson":{"key":"delegated","value":"from-a-member-with-no-node"},"warrant":"%s","authorProof":"%s"}' \
    "${WARRANT}" "${CREDENTIAL}")

check "POST intent on the open node" 200 \
    "$(status_of POST "${OPEN_URL}/admin-api/contexts/${CONTEXT}/intents" "${INTENT_BODY}")"

check "POST intent on the closed node" 401 \
    "$(status_of POST "${CLOSED_URL}/admin-api/contexts/${CONTEXT}/intents" "${INTENT_BODY}")"

# --- 3. Only those two routes ------------------------------------------------
#
# The assertion that matters most, and the one a scenario testing only the happy
# path would miss. `--public-intents` opens the intent pair and nothing else; a
# change that moved the whole admin router, or widened the ingress regex, would
# satisfy every check above and fail here.

for route in \
    "GET /admin-api/contexts" \
    "GET /admin-api/applications" \
    "GET /admin-api/identity" \
    "GET /admin-api/peers"
do
    _method=${route%% *}
    _path=${route#* }
    check "open node still refuses ${_method} ${_path}" 401 \
        "$(status_of "${_method}" "${OPEN_URL}${_path}")"
done

# A sibling of the exempted path, one segment along. If this answers, the route
# was opened by prefix rather than by shape.
check "open node still refuses the context's own record" 401 \
    "$(status_of GET "${OPEN_URL}/admin-api/contexts/${CONTEXT}")"

echo "== ${PASS} passed, ${FAIL} failed =="
[ "${FAIL}" -eq 0 ] || fail "the public-intents posture does not hold"
