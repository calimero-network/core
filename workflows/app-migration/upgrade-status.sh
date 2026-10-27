#!/bin/sh
#
# GET /admin-api/groups/<namespace>/upgrade/status on one node and check the answer.
# Usage: upgrade-status.sh <node> <namespace_id> <jq_assertion>
# curl rather than meroctl: the merod image ships no CLI for a `target: local` script.

. apps/scaffolding-e2e/scripts/account-api.sh

node="${1:?node}"
namespace_id="${2:?namespace id}"
assertion="${3:?jq assertion}"

url=$(node_url "${node}") || fail "no admin URL for ${node}"
response=$(curl -fsS "${url}/admin-api/groups/${namespace_id}/upgrade/status") \
    || fail "could not read the upgrade status from ${node}"
echo "${response}"
echo "${response}" | jq -e "${assertion}" >/dev/null || fail "${node} answer does not satisfy: ${assertion}"
