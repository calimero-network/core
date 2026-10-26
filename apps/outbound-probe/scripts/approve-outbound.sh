#!/bin/sh
#
# Sets what the context's namespace approves for the probe's publisher, as an
# admin of the namespace would: `PUT /admin-api/namespaces/{id}/outbound-approvals`.
# "manifest" approves every prefix the probe declares; "[]" revokes them all.
#
#   $1  the node's admin URL   $2  the context id   $3  the probe's application id
#   $4  the selection, as JSON ("\"manifest\"" or a list of prefixes)
set -eu

url="$1"
context_id="$2"
app_id="$3"
selection="$4"

namespace=$(curl -fsS "${url}/admin-api/contexts/${context_id}/group" | jq -r '.data // empty')
[ -n "${namespace}" ] || { echo "FAIL: no group for ${context_id}" >&2; exit 1; }

answer=$(curl -fsS -X PUT "${url}/admin-api/namespaces/${namespace}/outbound-approvals" \
    -H 'Content-Type: application/json' \
    -d "{\"applicationId\":\"${app_id}\",\"approve\":${selection}}")
echo "${answer}"

listed=$(curl -fsS "${url}/admin-api/namespaces/${namespace}/outbound-approvals")
echo "approvals now: ${listed}"
