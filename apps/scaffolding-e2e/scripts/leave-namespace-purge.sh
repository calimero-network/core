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

TOKEN=$(operator_token "${URL}" leave-namespace-purge) || exit 1

OUT=$(mktemp)
STATUS=$(curl -sS -o "${OUT}" -w '%{http_code}' -X POST \
    "${URL}/admin-api/namespaces/${NAMESPACE}/leave?purge=true" \
    -H "Authorization: Bearer ${TOKEN}")
BODY=$(cat "${OUT}")
rm -f "${OUT}"
[ "${STATUS}" = 200 ] || fail "the purging leave answered ${STATUS}: ${BODY}"
[ "$(echo "${BODY}" | jq -r '.data.purged')" = true ] || fail "the leave did not purge: ${BODY}"
echo "left ${NAMESPACE} with a purge"
