#!/bin/sh
#
# POST /admin-api/groups/<namespace>/upgrade/activate on one node and check the answer.
# Usage: activate-upgrade.sh <node> <namespace_id> <body_json> <want_status> [<jq_assertion>]
# curl rather than meroctl: the merod image ships no CLI for a `target: local` script.

. apps/scaffolding-e2e/scripts/account-api.sh

node="${1:?node}"
namespace_id="${2:?namespace id}"
body="${3:?body}"
want_status="${4:?expected status}"
assertion="${5:-}"

url=$(node_url "${node}") || fail "no admin URL for ${node}"
response=$(mktemp)
status=$(curl -sS -o "${response}" -w '%{http_code}' -X POST \
    -H 'Content-Type: application/json' -d "${body}" \
    "${url}/admin-api/groups/${namespace_id}/upgrade/activate") || fail "request to ${node} failed"
cat "${response}"
echo

[ "${status}" = "${want_status}" ] || fail "${node} answered ${status}, wanted ${want_status}"
if [ -n "${assertion}" ]; then
    jq -e "${assertion}" "${response}" >/dev/null || fail "${node} answer does not satisfy: ${assertion}"
fi
rm -f "${response}"
