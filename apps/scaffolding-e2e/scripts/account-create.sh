#!/bin/sh
#
# Enroll a node's device under a fresh account.
#
#     - name: Node-2 enrolls a device
#       type: script
#       script: scripts/account-create.sh
#       target: local
#       args: [ <container>, <namespace-id> ]
#
# Must run AFTER the node holds the namespace's scope key: the device link
# travels as an encrypted group op, so a keyless node cannot publish one. The
# node refuses with that reason rather than failing obscurely, but the ordering
# is the point — moving this before the context join deadlocks the bootstrap.
#
# Records the account and device ids for later steps to read.

set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <container> <namespace-id>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

container="$1"
namespace="$2"

resp=$(api_post "${container}" "namespaces/${namespace}/account" '{}')

account=$(echo "${resp}" | jq -r '.data.accountId')
device=$(echo "${resp}" | jq -r '.data.deviceId')
root_key=$(echo "${resp}" | jq -r '.data.accountRootKey')
nonce=$(echo "${resp}" | jq -r '.data.accountNonce')

if [ -z "${account}" ] || [ "${account}" = "null" ]; then
    echo "account create returned no accountId: ${resp}" >&2
    exit 1
fi

# The genesis halves are what a pairing device needs to mint its own id, and the
# device id is what a revocation names.
printf '%s\n' "${account}" > "$(state_file "${namespace}" account)"
printf '%s\n' "${device}"  > "$(state_file "${namespace}" first-device)"
printf '%s\n' "${root_key}" > "$(state_file "${namespace}" root-key)"
printf '%s\n' "${nonce}"   > "$(state_file "${namespace}" nonce)"

echo "enrolled ${container}: account=${account} device=${device}"
