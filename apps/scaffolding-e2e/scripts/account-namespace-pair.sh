#!/bin/sh
#
# Pair <new-node> onto <holder>'s account with nothing but the account namespace
# id, then prove the two nodes agree on it, the holder keeps it out of its
# namespace listing, and each side sees the new device bound in it.
#
# args: [ <holder>, <new-node> ]
set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <holder> <new-node>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
newnode="$2"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# Deterministic from the root, so the holder names it before anything created it.
identity=$(api "${holder}" GET "identity")
root_key=$(echo "${identity}" | jq -r '.data.accountRootPublicKey')
namespace=$(echo "${identity}" | jq -r '.data.accountNamespaceId // empty')
[ -n "${namespace}" ] || fail "the holder names no account namespace before pairing"

init=$(api "${newnode}" POST "account/pair-init" \
    "{\"accountRootPublicKey\":\"${root_key}\",\"accountNamespace\":\"${namespace}\",\"namespaces\":[]}")
device=$(echo "${init}" | jq -r '.data.deviceId')
kem=$(echo "${init}" | jq -r '.data.kemPublicKey')
sign=$(echo "${init}" | jq -r '.data.signPublicKey')
statement=$(echo "${init}" | jq -r '.data.statement')
code=$(echo "${init}" | jq -r '.data.confirmationCode')

complete=$(api "${holder}" POST "account/pair-complete" \
    "{\"deviceId\":\"${device}\",\"kemPublicKey\":\"${kem}\",\"signPublicKey\":\"${sign}\",\"statement\":\"${statement}\",\"confirmationCode\":\"${code}\",\"applications\":[]}")
[ "$(echo "${complete}" | jq -r '.data.keyDelivered')" = "true" ] \
    || fail "pair-complete delivered no key: ${complete}"

# Both sides name the same namespace.
recorded=$(api "${newnode}" GET "identity" | jq -r '.data.accountNamespaceId // empty')
[ "${recorded}" = "${namespace}" ] || fail "the device recorded '${recorded}', the holder names '${namespace}'"

# A project listing never shows it.
api "${holder}" GET "namespaces" \
    | jq -e --arg ns "${namespace}" 'all(.data[]; .namespaceId != $ns)' >/dev/null \
    || fail "the holder lists the account namespace as a project"

# The holder bound the device there.
api "${holder}" GET "account/devices" \
    | jq -e --arg d "${device}" --arg ns "${namespace}" \
        'any(.devices[]; .deviceId == $d and (.namespaces | index($ns)) != null)' >/dev/null \
    || fail "the holder does not show the device bound in the account namespace"

# The device folded the account namespace: it resolves its own binding there.
# Polled, because it learns of the publish from the holder's beacons.
tries=45
while [ "${tries}" -gt 0 ]; do
    if api "${newnode}" GET "account/devices" \
        | jq -e --arg d "${device}" --arg ns "${namespace}" \
            'any(.devices[]; .deviceId == $d and .isSelf and (.namespaces | index($ns)) != null)' >/dev/null 2>&1; then
        echo "device ${device} follows account namespace ${namespace} on both nodes"
        exit 0
    fi
    tries=$((tries - 1))
    sleep 2
done
fail "the device never resolved its own binding in the account namespace"
