#!/bin/sh
#
# What a revoked node answers to `GET /admin-api/identity`.
#
#     - name: The revoked device no longer presents itself as Alice's
#       type: script
#       script: scripts/account-device-spent.sh
#       target: local
#       args: [ <device-node>, <spent-device-id>, <holder-account> ]
#
# A revocation is terminal and one device row serves every namespace, so a device
# revoked anywhere holds an id that is spent everywhere: re-enrolling the machine
# mints a fresh one. Presenting the old id would name a device no peer will admit
# and - for a paired node - an account it no longer speaks for, which is a
# machine reporting somebody else's identity as its own.
#
# TWO answers are correct here and the scenario cannot decide which it will get,
# so both are accepted and the assertion is what they have in common. The node
# falls back to its own account root, and whether it holds one depends on whether
# anything happened to mint one: pairing does not, and the paths that do
# (certifying a device, repairing one, a join) are not things a paired device
# does. So it answers either `404` - nothing left to report - or its own account
# with no device. What must never happen is either of the two negatives below.
#
# Polled, because the tombstone reaches this node by sync rather than by the call
# that made it.

set -eu

ATTEMPTS=30
INTERVAL=2

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <device-node> <spent-device-id> <holder-account>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

devicenode="$1"
spent="$2"
holder_account="$3"

attempt=0
while :; do
    attempt=$((attempt + 1))
    status=$(api_status "${devicenode}" GET "identity")

    if [ "${status}" = "404" ]; then
        echo "${devicenode} reports no identity at all: its device is spent and it holds no root of its own"
        exit 0
    fi

    if [ "${status}" = "200" ]; then
        # Tolerated rather than asserted: the tombstone can land between the two
        # calls, turning this into the `404` the branch above accepts anyway.
        identity=$(api "${devicenode}" GET "identity") || identity=''
    fi
    if [ "${status}" = "200" ] && [ -n "${identity:-}" ]; then
        device=$(echo "${identity}" | jq -r '.data.deviceId // "null"')
        account=$(echo "${identity}" | jq -r '.data.accountId')
        if [ "${device}" != "${spent}" ]; then
            # The first negative: a spent id must not be presented as this
            # node's. The second: nor may the account it adopted, which it can no
            # longer speak for - and the fallback names this node's own root, so
            # the two ids differ.
            if [ "${account}" = "${holder_account}" ]; then
                echo "${devicenode} still claims account ${account} after its device was revoked: ${identity}" >&2
                exit 1
            fi
            echo "${devicenode} fell back to its own account ${account}, device ${device}"
            exit 0
        fi
    fi

    if [ "${attempt}" -ge "${ATTEMPTS}" ]; then
        echo "${devicenode} still presents the revoked device ${spent} (last status ${status})" >&2
        exit 1
    fi
    sleep "${INTERVAL}"
done
