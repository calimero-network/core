#!/bin/sh
#
# Pair <new-node> onto <holder>'s account, following the holder's ACCOUNT
# NAMESPACE as well as any project namespaces named, and scoped to the
# applications named. merobox's account_pair step has no field for the account
# namespace, and a device that does not follow it never folds the registry.
#
# Returns only once the new node has folded what the holder wrote there.
#
# args: [ <holder>, <new-node>, <namespace-csv|->, <application-csv|-> ]
set -eu

if [ "$#" -ne 4 ]; then
    echo "usage: $0 <holder> <new-node> <namespace-csv|-> <application-csv|->" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
newnode="$2"
namespaces="$3"
applications="$4"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# `-` for none, so an empty positional cannot be mistaken for a missing one.
json_array() {
    if [ "$1" = "-" ]; then
        echo '[]'
    else
        echo "$1" | jq -R 'split(",")'
    fi
}

identity=$(api "${holder}" GET "identity")
root_key=$(echo "${identity}" | jq -r '.data.accountRootPublicKey')
account_namespace=$(echo "${identity}" | jq -r '.data.accountNamespaceId // empty')
[ -n "${account_namespace}" ] || fail "the holder names no account namespace"

init=$(api "${newnode}" POST "account/pair-init" \
    "$(jq -nc --arg root "${root_key}" --arg ns "${account_namespace}" \
        --argjson namespaces "$(json_array "${namespaces}")" \
        '{accountRootPublicKey:$root,accountNamespace:$ns,namespaces:$namespaces}')")
device=$(echo "${init}" | jq -r '.data.deviceId')
kem=$(echo "${init}" | jq -r '.data.kemPublicKey')
sign=$(echo "${init}" | jq -r '.data.signPublicKey')
statement=$(echo "${init}" | jq -r '.data.statement')
code=$(echo "${init}" | jq -r '.data.confirmationCode')

complete=$(api "${holder}" POST "account/pair-complete" \
    "$(jq -nc --arg d "${device}" --arg kem "${kem}" --arg sign "${sign}" \
        --arg statement "${statement}" --arg code "${code}" \
        --argjson applications "$(json_array "${applications}")" \
        '{deviceId:$d,kemPublicKey:$kem,signPublicKey:$sign,statement:$statement,confirmationCode:$code,applications:$applications}')")
[ "$(echo "${complete}" | jq -r '.data.keyDelivered')" = "true" ] \
    || fail "pair-complete delivered no key: ${complete}"

# The barrier. A scope can only have come from the replicated registry, so
# reading its own scope back proves this node folded the LAST op the holder
# published there - and therefore every earlier one. With no scope named there
# is no such field, so the weaker "my own link is here" is all there is.
tries=45
while [ "${tries}" -gt 0 ]; do
    listing=$(api "${newnode}" GET "account/devices" 2>/dev/null || true)
    if [ "${applications}" = "-" ]; then
        if echo "${listing}" | jq -e --arg d "${device}" --arg ns "${account_namespace}" \
            'any(.devices[]; .deviceId == $d and .isSelf and (.namespaces | index($ns)) != null)' \
            >/dev/null 2>&1; then
            echo "device ${device} follows account namespace ${account_namespace}"
            exit 0
        fi
    elif echo "${listing}" | jq -e --arg d "${device}" \
        --argjson want "$(json_array "${applications}")" \
        'any(.devices[]; .deviceId == $d and .isSelf and .applications == $want)' \
        >/dev/null 2>&1; then
        echo "device ${device} reads scope ${applications} back out of the registry"
        exit 0
    fi
    tries=$((tries - 1))
    sleep 2
done
fail "the device never folded the account namespace"
