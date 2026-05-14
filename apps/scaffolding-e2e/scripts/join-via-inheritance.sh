#!/bin/sh
#
# Call `POST /admin-api/groups/:group_id/join-via-inheritance` against a
# merobox-managed node's host-exposed admin API. Merobox numbers nodes
# starting at 1; the admin port for node N is 2527+N (node-1 → 2528,
# node-2 → 2529, …).
#
# Usage:
#     - name: Node-2 joins subgroup via inheritance
#       type: script
#       script: apps/scaffolding-e2e/scripts/join-via-inheritance.sh
#       target: local
#       args:
#         - 2                          # node index
#         - "deadbeef…"               # group_id (hex, 32 bytes)
#
# Exits non-zero if curl returns non-2xx so the workflow fails when the
# endpoint isn't reachable or refuses the request.

set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <node-index> <group-id-hex>" >&2
    exit 1
fi

node_index="$1"
group_id="$2"
port=$((2527 + node_index))
url="http://localhost:${port}/admin-api/groups/${group_id}/join-via-inheritance"

echo "POST ${url}"
response=$(curl -sS -X POST -w "\n%{http_code}" "${url}")
body=$(printf '%s\n' "${response}" | sed '$d')
status=$(printf '%s\n' "${response}" | tail -n 1)

echo "status: ${status}"
echo "body: ${body}"

case "${status}" in
    2*) ;;
    *) echo "join-via-inheritance failed (HTTP ${status})" >&2; exit 1 ;;
esac
