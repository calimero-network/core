#!/bin/sh
#
# Revoke the paired device recorded by `account-pair.sh`.
#
#     - name: Admin revokes node-3's device
#       type: script
#       script: scripts/account-revoke.sh
#       target: local
#       args: [ <admin-container>, <namespace-id> ]
#
# Run as an ADMIN. Only an admin's revocation rotates the scope key, and without
# that rotation the revoked device keeps the key it already holds — it stops
# writing but goes on reading, which is the failure this whole scenario exists
# to catch. The script therefore asserts the rotation happened rather than
# trusting the revocation alone.

set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <admin-container> <namespace-id>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

admin="$1"
namespace="$2"
device=$(cat "$(state_file "${namespace}" paired-device)")

resp=$(api_post "${admin}" "namespaces/${namespace}/account/revoke" \
    "{\"deviceId\":\"${device}\"}")

rotated=$(echo "${resp}" | jq -r '.data.keyRotated')
if [ "${rotated}" != "true" ]; then
    echo "revoked ${device} but the scope key did NOT rotate — the device can \
still read: ${resp}" >&2
    exit 1
fi

echo "revoked ${device} and rotated the scope key"
