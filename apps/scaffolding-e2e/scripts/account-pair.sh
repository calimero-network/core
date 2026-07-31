#!/bin/sh
#
# Pair a second node onto an existing account — the full two-way exchange.
#
#     - name: Pair node-3 onto Alice's account
#       type: script
#       script: scripts/account-pair.sh
#       target: local
#       args: [ <holder-container>, <new-container>, <namespace-id> ]
#
# Two commands, not one, and the ordering is forced rather than stylistic: the
# new device cannot mint its DeviceId until it knows the account (the id is
# H(account ‖ nonce)), and the holder cannot certify that device until it knows
# the id and the two keys the new node minted. So:
#
#   1. pair-init on the NEW node, given the account's genesis  -> device id,
#      KEM key, signing key
#   2. pair-complete on the HOLDER, given those three          -> link + the
#      scope key wrapped for the new device
#
# Step 2 is what makes the device usable at all. Without it the link lands and
# the new device holds no key, so it can write nothing it can read back.
#
# The new node is deliberately NOT a namespace member. Its entire right to
# participate comes from being a device of the account.

set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <holder-container> <new-container> <namespace-id>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
newnode="$2"
namespace="$3"

root_key=$(cat "$(state_file "${namespace}" root-key)")
nonce=$(cat "$(state_file "${namespace}" nonce)")

init=$(api_post "${newnode}" "namespaces/${namespace}/account/pair-init" \
    "{\"accountRootKey\":\"${root_key}\",\"accountNonce\":\"${nonce}\"}")

device=$(echo "${init}" | jq -r '.data.deviceId')
kem=$(echo "${init}" | jq -r '.data.kemPublicKey')
sign=$(echo "${init}" | jq -r '.data.signPublicKey')

if [ -z "${device}" ] || [ "${device}" = "null" ]; then
    echo "pair-init returned no deviceId: ${init}" >&2
    exit 1
fi

complete=$(api_post "${holder}" "namespaces/${namespace}/account/pair-complete" \
    "{\"deviceId\":\"${device}\",\"kemPublicKey\":\"${kem}\",\"signPublicKey\":\"${sign}\"}")

delivered=$(echo "${complete}" | jq -r '.data.keyDelivered')
paired_account=$(echo "${complete}" | jq -r '.data.accountId')

# A link without the key is not a working device — it can author but reads
# nothing. Fail loudly here rather than let the next write fail for a reason
# that looks unrelated.
if [ "${delivered}" != "true" ]; then
    echo "paired ${newnode} but the scope key was NOT delivered: ${complete}" >&2
    exit 1
fi

printf '%s\n' "${device}" > "$(state_file "${namespace}" paired-device)"

echo "paired ${newnode}: account=${paired_account} device=${device}"
