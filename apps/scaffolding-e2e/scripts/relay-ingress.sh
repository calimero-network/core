#!/bin/sh
#
# A keyholder reaches a proxy-mode relay the way it reaches a fleet TEE node:
# through Traefik and a standalone mero-auth, never the node's own port.
#
# `delegated-session.yml` proves device-key login, reads and delegated writes
# against a node in EMBEDDED auth mode, where merod checks every token itself.
# A fleet relay does not run that way. It runs merod in PROXY mode behind
# Traefik and mero-auth, and there core installs no guard at all: until
# `server.proxy_identity` the node could not tell one keyholder from another,
# so the caller-scoped listings answered node-wide -- every tenant's rows -- and
# `/query` refused outright. This script stands up that topology in front of a
# node started with `--delegated-access --proxy-identity` and runs the SAME
# assertions `delegated-session.yml` makes, through it:
#
#   1. mero-auth serves `account_proof`, naming the relay's own key;
#   2. the relay's own port, reached directly, answers for whoever the identity
#      headers name -- the control that shows the scoping below comes from them;
#   3. through the ingress: a device-key session reads, writes and replays
#      exactly as it does against an embedded node (delegated-session.sh);
#   4. through the ingress: the member is served its own contexts and
#      namespaces and the stranger none of them (delegated-read-scope.sh);
#   5. through the ingress: a stranger that forges the member's identity
#      headers is still served as the stranger, and a forged header with no
#      token gets nothing at all.
#
# Usage: relay-ingress.sh <relay_node> <node_key> <relay_account> <context> <namespace>
#                         <member_account> <member_secret> <member_credential>
#                         <stranger_secret> <stranger_credential>
#
# Needs `mero-auth` (MERO_AUTH_BIN, default ../../target/debug/mero-auth) and
# `traefik` (TRAEFIK_BIN, default on PATH) on the machine running merobox.

set -e

. "$(dirname "$0")/account-api.sh"

RELAY_NODE="$1"
NODE_KEY="$2"
RELAY_ACCOUNT="$3"
CONTEXT="$4"
NAMESPACE="$5"
MEMBER_ACCOUNT="$6"
MEMBER_SECRET="$7"
MEMBER_CREDENTIAL="$8"
STRANGER_SECRET="$9"
shift 9
STRANGER_CREDENTIAL="$1"

[ -n "${STRANGER_CREDENTIAL}" ] || fail "usage: relay-ingress.sh <relay_node> <node_key> <relay_account> <context> <namespace> <member_account> <member_secret> <member_credential> <stranger_secret> <stranger_credential>"

HERE=$(cd "$(dirname "$0")" && pwd)
INGRESS_DIR="${HERE}/../relay-ingress"
MERO_AUTH_BIN="${MERO_AUTH_BIN:-../../target/debug/mero-auth}"
TRAEFIK_BIN="${TRAEFIK_BIN:-traefik}"
INGRESS_PORT="${RELAY_INGRESS_PORT:-4480}"
AUTH_PORT="${RELAY_AUTH_PORT:-4481}"

[ -x "${MERO_AUTH_BIN}" ] || fail "no mero-auth at ${MERO_AUTH_BIN} (set MERO_AUTH_BIN)"
command -v "${TRAEFIK_BIN}" >/dev/null 2>&1 || fail "no traefik at ${TRAEFIK_BIN} (set TRAEFIK_BIN)"

# The node's own port, resolved BEFORE INGRESS_URL is set: after that,
# node_url answers with the ingress.
RELAY_URL=$(node_url "${RELAY_NODE}") || fail "could not resolve ${RELAY_NODE}'s URL"
AUTH_URL="http://127.0.0.1:${AUTH_PORT}"
INGRESS="http://127.0.0.1:${INGRESS_PORT}"

WORK="$(pwd)/data/relay-ingress"
rm -rf "${WORK}"
mkdir -p "${WORK}"

AUTH_PID=""
TRAEFIK_PID=""
cleanup() {
    [ -z "${TRAEFIK_PID}" ] || kill "${TRAEFIK_PID}" 2>/dev/null || true
    [ -z "${AUTH_PID}" ] || kill "${AUTH_PID}" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

show_logs() {
    echo "--- mero-auth log ---" >&2
    tail -n 60 "${WORK}/mero-auth.log" >&2 || true
    echo "--- traefik log ---" >&2
    tail -n 60 "${WORK}/traefik.log" >&2 || true
}

# Wait until a URL answers with the given status, or give up naming it.
wait_for() {
    _url="$1"
    _want="$2"
    _what="$3"
    _i=0
    while [ "${_i}" -lt 60 ]; do
        _code=$(curl -s -o /dev/null -w '%{http_code}' "${_url}" || true)
        [ "${_code}" = "${_want}" ] && return 0
        _i=$((_i + 1))
        sleep 1
    done
    show_logs
    fail "${_what} never answered ${_want} at ${_url} (last: ${_code})"
}

# --- mero-auth, as a fleet relay runs it -------------------------------------
#
# The same two tables mero-tee's `mero-auth-start` appends to the image's baked
# config: the provider on, named by the relay's own device signing key, and any
# audience accepted. `node_key` is the value the scenario read from the relay's
# `/admin-api/identity`, so a statement addressed to any other node is refused.
cat > "${WORK}/auth.toml" <<EOF
listen_addr = "127.0.0.1:${AUTH_PORT}"

[jwt]
issuer = "relay-ingress-e2e"

[storage]
type = "rocksdb"
path = "${WORK}/auth_db"

[providers]
account_proof = true

[account_proof]
node_key = "${NODE_KEY}"
allowed_audiences = []
EOF

"${MERO_AUTH_BIN}" --config "${WORK}/auth.toml" > "${WORK}/mero-auth.log" 2>&1 &
AUTH_PID=$!
wait_for "${AUTH_URL}/auth/providers" 200 "mero-auth"

# --- Traefik, with the relay's routing ---------------------------------------
sed -e "s#@RELAY_URL@#${RELAY_URL}#g" -e "s#@AUTH_URL@#${AUTH_URL}#g" \
    "${INGRESS_DIR}/routing.yml" > "${WORK}/routing.yml"
sed -e "s#@INGRESS_PORT@#${INGRESS_PORT}#g" -e "s#@ROUTING_FILE@#${WORK}/routing.yml#g" \
    -e "s#@LOG_FILE@#${WORK}/traefik.log#g" \
    "${INGRESS_DIR}/traefik.yml" > "${WORK}/traefik.yml"

"${TRAEFIK_BIN}" --configFile="${WORK}/traefik.yml" > "${WORK}/traefik.out" 2>&1 &
TRAEFIK_PID=$!
wait_for "${INGRESS}/admin-api/health" 200 "the ingress"
echo "ingress up: ${INGRESS} -> ${RELAY_URL}, auth ${AUTH_URL}"

# --- 1. The provider is on, through the ingress -------------------------------
#
# mero-auth falls back to its DEFAULT config, silently, when it cannot parse the
# one it was given -- which enables no account_proof. So this is asserted rather
# than assumed: every step below would otherwise fail for a reason that reads as
# a scoping bug.
PROVIDERS=$(curl -sS "${INGRESS}/auth/providers")
echo "${PROVIDERS}" | grep -q 'account_proof' \
    || { show_logs; fail "the ingress's mero-auth does not serve account_proof: ${PROVIDERS}"; }
echo "ok device-key login is served through the ingress"

# --- 2. The control: the relay's own port trusts the identity headers ---------
#
# Reached directly, the relay takes the caller's account from X-Auth-Account
# (`server.proxy_identity`). That is exactly why only the ingress may write it,
# and it is what shows the scoping asserted through the ingress below comes
# from the headers mero-auth vouched for and nothing else.
direct_contexts() {
    if [ -n "$1" ]; then
        curl -sS -o "${WORK}/direct.json" -w '%{http_code}' \
            -H "X-Auth-Account: $1" "${RELAY_URL}/admin-api/contexts"
    else
        curl -sS -o "${WORK}/direct.json" -w '%{http_code}' "${RELAY_URL}/admin-api/contexts"
    fi
}

CODE=$(direct_contexts "")
{ [ "${CODE}" = "200" ] && grep -q "${CONTEXT}" "${WORK}/direct.json"; } \
    || fail "the relay's own port, with no identity, did not answer node-wide (${CODE}): $(cat "${WORK}/direct.json")"
echo "ok direct, no identity: node-wide, as proxy mode always answered"

CODE=$(direct_contexts "${MEMBER_ACCOUNT}")
{ [ "${CODE}" = "200" ] && grep -q "${CONTEXT}" "${WORK}/direct.json"; } \
    || fail "the relay did not serve the member named by X-Auth-Account (${CODE}): $(cat "${WORK}/direct.json")"
echo "ok direct, X-Auth-Account=member: lists the member's context"

# Any well-formed account in no group. All-f is not a content address anyone can
# reach, which is the point: it is nobody.
NOBODY=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
CODE=$(direct_contexts "${NOBODY}")
[ "${CODE}" = "200" ] || fail "the relay refused a well-formed X-Auth-Account (${CODE}): $(cat "${WORK}/direct.json")"
if grep -q "${CONTEXT}" "${WORK}/direct.json"; then
    fail "the relay served an account in no group the member's context: $(cat "${WORK}/direct.json")"
fi
echo "ok direct, X-Auth-Account=nobody: scoped to nothing"

CODE=$(direct_contexts "not-an-account")
[ "${CODE}" = "401" ] || fail "an unparseable X-Auth-Account was not refused (${CODE}): $(cat "${WORK}/direct.json")"
echo "ok direct, X-Auth-Account=garbage: 401, never read as nobody"

# --- 3 and 4. The embedded-mode assertions, through the ingress --------------
INGRESS_URL="${INGRESS}"
export INGRESS_URL

sh "${HERE}/delegated-session.sh" "${RELAY_NODE}" "${CONTEXT}" \
    "${MEMBER_SECRET}" "${MEMBER_CREDENTIAL}" "${NODE_KEY}" "${RELAY_ACCOUNT}" \
    || { show_logs; fail "the device-key session did not hold through the ingress"; }

sh "${HERE}/delegated-read-scope.sh" "${RELAY_NODE}" "${NODE_KEY}" "${CONTEXT}" "${NAMESPACE}" \
    "${MEMBER_SECRET}" "${MEMBER_CREDENTIAL}" "${STRANGER_SECRET}" "${STRANGER_CREDENTIAL}" \
    || { show_logs; fail "read scoping did not hold through the ingress"; }

# --- 5. Forged identity headers do not survive the ingress --------------------
#
# The stranger holds a perfectly valid session and claims to be the member. On
# a guarded route Traefik replaces both headers with mero-auth's answer, so the
# node sees the stranger and lists nothing of the member's.
STRANGER_TOKEN=$(mint_session "${INGRESS}" "${NODE_KEY}" "${STRANGER_SECRET}" "${STRANGER_CREDENTIAL}")
[ -n "${STRANGER_TOKEN}" ] || fail "the stranger could not log in through the ingress"
CODE=$(curl -sS -o "${WORK}/forged.json" -w '%{http_code}' \
    -H "Authorization: Bearer ${STRANGER_TOKEN}" \
    -H "X-Auth-Account: ${MEMBER_ACCOUNT}" \
    -H "X-Auth-Device: $(printf '0%.0s' $(seq 1 64))" \
    "${INGRESS}/admin-api/contexts")
[ "${CODE}" = "200" ] || fail "the stranger's forged request was not answered as the stranger (${CODE}): $(cat "${WORK}/forged.json")"
if grep -q "${CONTEXT}" "${WORK}/forged.json"; then
    fail "a stranger naming the member in X-Auth-Account was served the member's context: $(cat "${WORK}/forged.json")"
fi
echo "ok stranger forging the member's X-Auth-Account is still the stranger"

for path in /admin-api/contexts /admin-api/namespaces; do
    CODE=$(curl -sS -o /dev/null -w '%{http_code}' \
        -H "X-Auth-Account: ${MEMBER_ACCOUNT}" "${INGRESS}${path}")
    [ "${CODE}" = "401" ] || fail "a forged X-Auth-Account with no token got ${CODE} from ${path}, expected 401"
    echo "ok forged X-Auth-Account with no token on ${path}: 401"
done

echo "PASS: behind a proxy, a device-key session is scoped to its own account, and no client can name another"
