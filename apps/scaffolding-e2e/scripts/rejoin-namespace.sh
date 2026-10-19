#!/bin/sh
#
# Rejoin a namespace this node removed, with no invitation: the request body
# names none, which no typed merobox step can send.
#
# Usage: rejoin-namespace.sh <node> <namespace_id>

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
NAMESPACE="$2"

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

ANSWER=$(curl -sS -w '\n%{http_code}' -X POST "${URL}/admin-api/namespaces/${NAMESPACE}/join" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H 'Content-Type: application/json' \
    -d '{}')
STATUS=$(echo "${ANSWER}" | tail -1)
BODY=$(echo "${ANSWER}" | sed '$d')
[ "${STATUS}" = "200" ] || fail "the rejoin answered ${STATUS}: ${BODY}"
[ "$(echo "${BODY}" | jq -r '.data.namespaceId')" = "${NAMESPACE}" ] \
    || fail "the rejoin named another namespace: ${BODY}"
echo "rejoined ${NAMESPACE}"
