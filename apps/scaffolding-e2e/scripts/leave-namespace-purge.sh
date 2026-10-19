#!/bin/sh
#
# Leave a namespace with `?purge=true`, which no merobox step can send yet.
#
# Usage: leave-namespace-purge.sh <node> <namespace_id>

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
NAMESPACE="$2"

[ -n "${NODE}" ] || fail "usage: leave-namespace-purge.sh <node> <namespace_id>"
[ -n "${NAMESPACE}" ] || fail "no namespace id given"
command -v jq >/dev/null || fail "jq is required to read the login"

URL=$(node_url "${NODE}") || fail "could not resolve ${NODE}'s URL"

LOGIN=$(curl -sS -X POST "${URL}/auth/token" \
    -H 'Content-Type: application/json' \
    -d "$(printf '{"auth_method":"user_password","public_key":"leave-namespace-purge","client_name":"%s","timestamp":%s,"provider_data":{"username":"%s","password":"%s"}}' \
        "${URL}" "$(date +%s)" "${MERO_AUTH_ADMIN_USER:-dev}" "${MERO_AUTH_ADMIN_PASSWORD:-dev-password}")")
TOKEN=$(echo "${LOGIN}" | jq -r '.data.access_token // .access_token // empty')
[ -n "${TOKEN}" ] || fail "the operator could not log in: ${LOGIN}"

OUT=$(mktemp)
STATUS=$(curl -sS -o "${OUT}" -w '%{http_code}' -X POST \
    "${URL}/admin-api/namespaces/${NAMESPACE}/leave?purge=true" \
    -H "Authorization: Bearer ${TOKEN}")
BODY=$(cat "${OUT}")
rm -f "${OUT}"
[ "${STATUS}" = 200 ] || fail "the purging leave answered ${STATUS}: ${BODY}"
echo "left ${NAMESPACE} with a purge"
