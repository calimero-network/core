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

group=$(curl -sS -w '\n%{http_code}' "${url}/admin-api/contexts/${context_id}/group")
case "$(printf '%s' "${group}" | tail -n 1)" in
    2??) ;;
    *) echo "FAIL: reading ${context_id}'s group: ${group}" >&2; exit 1 ;;
esac
namespace=$(printf '%s' "${group}" | sed '$d' | jq -r '.data // empty')
[ -n "${namespace}" ] || { echo "FAIL: no group for ${context_id}" >&2; exit 1; }

body="{\"applicationId\":\"${app_id}\",\"approve\":${selection}}"
answer=$(curl -sS -w '\n%{http_code}' -X PUT "${url}/admin-api/namespaces/${namespace}/outbound-approvals" \
    -H 'Content-Type: application/json' -d "${body}")
status=$(printf '%s' "${answer}" | tail -n 1)
echo "PUT ${body} -> ${status}: $(printf '%s' "${answer}" | sed '$d')"
case "${status}" in
    2??) ;;
    *) echo "FAIL: the namespace ${namespace} refused the approval" >&2; exit 1 ;;
esac

listed=$(curl -fsS "${url}/admin-api/namespaces/${namespace}/outbound-approvals")
echo "approvals now: ${listed}"
