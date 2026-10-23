#!/bin/sh
#
# Hand a group to one of its admins, which no typed merobox step can send.
# Usage: transfer-ownership.sh <node> <group_id> <new_owner_account> [expected_status]

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
GROUP="$2"
NEW_OWNER="$3"
EXPECTED="${4:-200}"

[ -n "${NODE}" ] || fail "usage: transfer-ownership.sh <node> <group_id> <new_owner_account> [expected_status]"
[ -n "${GROUP}" ] || fail "no group id given"
[ -n "${NEW_OWNER}" ] || fail "no new owner given"
command -v jq >/dev/null || fail "jq is required to read the login"

URL=$(node_url "${NODE}") || fail "could not resolve ${NODE}'s URL"

TOKEN=$(operator_token "${URL}" transfer-ownership) || exit 1

OUT=$(mktemp)
STATUS=$(curl -sS -o "${OUT}" -w '%{http_code}' -X POST \
    "${URL}/admin-api/groups/${GROUP}/transfer-ownership" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H 'Content-Type: application/json' \
    -d "$(printf '{"newOwner":"%s"}' "${NEW_OWNER}")")
BODY=$(cat "${OUT}")
rm -f "${OUT}"
[ "${STATUS}" = "${EXPECTED}" ] || fail "the transfer answered ${STATUS}, wanted ${EXPECTED}: ${BODY}"
echo "transfer of ${GROUP} to ${NEW_OWNER} answered ${STATUS}"
