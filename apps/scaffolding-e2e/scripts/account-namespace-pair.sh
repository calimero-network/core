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

here="$(dirname "$0")"
. "${here}/account-api.sh"

holder="$1"
newnode="$2"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# Deterministic from the root, so the holder names it before anything created it.
namespace=$(api "${holder}" GET "identity" | jq -r '.data.accountNamespaceId // empty')
[ -n "${namespace}" ] || fail "the holder names no account namespace before pairing"

# The pairing itself, up to and including the phone folding its own binding in
# the account namespace. No project namespace, no scope.
"${here}/account-namespace-pair-into.sh" "${holder}" "${newnode}" - -

identity=$(api "${newnode}" GET "identity")
device=$(echo "${identity}" | jq -r '.data.deviceId')

# Both sides name the same namespace.
recorded=$(echo "${identity}" | jq -r '.data.accountNamespaceId // empty')
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

echo "device ${device} follows account namespace ${namespace} on both nodes"
