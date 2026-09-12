#!/bin/sh
#
# Pair <new-node> onto <holder>'s account, scoped to one application, with
# nothing but the account namespace id, then prove the new node reads its OWN
# scope back out of that namespace. It holds no certificate cache, so the scope
# can only have reached it as replicated state.
#
# args: [ <holder>, <new-node>, <application-id> ]
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <holder> <new-node> <application-id>" >&2
    exit 1
fi

here="$(dirname "$0")"
. "${here}/account-api.sh"

holder="$1"
newnode="$2"
app="$3"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

identity=$(api "${holder}" GET "identity")
namespace=$(echo "${identity}" | jq -r '.data.accountNamespaceId // empty')
holder_device=$(echo "${identity}" | jq -r '.data.deviceId')
[ -n "${namespace}" ] || fail "the holder names no account namespace"

# The pairing itself, up to and including the phone reading its own scope back
# out of the registry - the assertion this scenario exists for.
"${here}/account-namespace-pair-into.sh" "${holder}" "${newnode}" - "${app}"

device=$(api "${newnode}" GET "identity" | jq -r '.data.deviceId')

# The holder has always been able to say this, from the certificate it signed.
# Asserted so a registry that replaced the cache cannot quietly lose the scope.
api "${holder}" GET "account/devices" \
    | jq -e --arg d "${device}" --arg a "${app}" \
        'any(.devices[]; .deviceId == $d and (.applications | index($a)) != null)' >/dev/null \
    || fail "the holder does not report the phone as scoped to ${app}"

# Corroborating, and already true before the registry: the holder's own device is
# bound in the account namespace by its genesis, so the binding scan finds it.
api "${newnode}" GET "account/devices" \
    | jq -e --arg d "${holder_device}" 'any(.devices[]; .deviceId == $d)' >/dev/null \
    || fail "the phone does not see the holder's device"

echo "device ${device} reads its own scope ${app} out of account namespace ${namespace}"
