#!/bin/sh
#
# Ask a node for an invitation that targets one group, and fail unless it answered as expected.
#
# Usage: group-invite.sh <node> <group_id> <accepted|refused>

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
GROUP="$2"
EXPECT="$3"

[ -n "${NODE}" ] && [ -n "${GROUP}" ] || fail "usage: group-invite.sh <node> <group_id> <accepted|refused>"
case "${EXPECT}" in accepted | refused) ;; *) fail "expectation must be accepted or refused" ;; esac
command -v jq >/dev/null || fail "jq is required to read the login"

URL=$(node_url "${NODE}") || fail "could not resolve ${NODE}'s URL"
TOKEN=$(operator_token "${URL}" group-invite) || exit 1

STATUS=$(curl -sS -o /dev/null -w '%{http_code}' -X POST "${URL}/admin-api/groups/${GROUP}/invite" \
    -H "Authorization: Bearer ${TOKEN}" -H 'Content-Type: application/json' -d '{}')
echo "an invitation targeting ${GROUP} answered ${STATUS}"
case "${STATUS}" in
    2??) GOT=accepted ;;
    000) fail "${NODE} did not answer" ;;
    *) GOT=refused ;;
esac
[ "${GOT}" = "${EXPECT}" ] || fail "the invitation was ${GOT}, expected ${EXPECT}"
