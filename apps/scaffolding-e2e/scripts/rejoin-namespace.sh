#!/bin/sh
#
# Rejoin a removed namespace with no invitation, which no typed merobox step can send.
# Usage: rejoin-namespace.sh <node> <namespace_id> [expected_status]

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
NAMESPACE="$2"
EXPECTED="${3:-200}"

[ -n "${NODE}" ] || fail "usage: rejoin-namespace.sh <node> <namespace_id>"
[ -n "${NAMESPACE}" ] || fail "no namespace id given"
command -v jq >/dev/null || fail "jq is required to read the answer"

URL=$(node_url "${NODE}") || fail "could not resolve ${NODE}'s URL"

LOGIN=$(curl -sS -X POST "${URL}/auth/token" \
    -H 'Content-Type: application/json' \
    -d "$(printf '{"auth_method":"user_password","public_key":"rejoin-namespace","client_name":"%s","timestamp":%s,"provider_data":{"username":"%s","password":"%s"}}' \
        "${URL}" "$(date +%s)" "${MERO_AUTH_ADMIN_USER:-dev}" "${MERO_AUTH_ADMIN_PASSWORD:-dev-password}")")
TOKEN=$(echo "${LOGIN}" | jq -r '.data.access_token // .access_token // empty')
[ -n "${TOKEN}" ] || fail "the operator could not log in: ${LOGIN}"

# A refusal is awaited: the leave that causes it reaches this node by sync.
ATTEMPTS=1
[ "${EXPECTED}" = "200" ] || ATTEMPTS=20
for _ in $(seq "${ATTEMPTS}"); do
    ANSWER=$(curl -sS -w '\n%{http_code}' -X POST "${URL}/admin-api/namespaces/${NAMESPACE}/join" \
        -H "Authorization: Bearer ${TOKEN}" \
        -H 'Content-Type: application/json' \
        -d '{}')
    STATUS=$(echo "${ANSWER}" | tail -1)
    BODY=$(echo "${ANSWER}" | sed '$d')
    [ "${STATUS}" = "${EXPECTED}" ] && break
    sleep 3
done
[ "${STATUS}" = "${EXPECTED}" ] || fail "the rejoin answered ${STATUS}, wanted ${EXPECTED}: ${BODY}"
[ "${EXPECTED}" = "200" ] || { echo "refused ${NAMESPACE} with ${STATUS}"; exit 0; }
[ "$(echo "${BODY}" | jq -r '.data.namespaceId')" = "${NAMESPACE}" ] \
    || fail "the rejoin named another namespace: ${BODY}"
echo "rejoined ${NAMESPACE}"
