#!/bin/sh
# Publish a TeeAuthoringPolicySet on a namespace root.
#
# merobox has no step for this op yet, so the workflow calls the admin route
# directly. Usage: set-tee-authoring-policy.sh <admin-url> <namespace-id> <mrtd>...
set -eu

url="$1"
group_id="$2"
shift 2

mrtds=""
for mrtd in "$@"; do
    mrtds="${mrtds:+$mrtds,}\"$mrtd\""
done

curl -fsS -X PUT \
    -H 'Content-Type: application/json' \
    -d "{\"allowedMrtd\":[${mrtds}]}" \
    "${url}/admin-api/groups/${group_id}/settings/tee-authoring-policy"
echo
