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

. "$(dirname "$0")/account-api.sh"

holder="$1"
newnode="$2"
app="$3"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

identity=$(api "${holder}" GET "identity")
root_key=$(echo "${identity}" | jq -r '.data.accountRootPublicKey')
namespace=$(echo "${identity}" | jq -r '.data.accountNamespaceId // empty')
holder_device=$(echo "${identity}" | jq -r '.data.deviceId')
[ -n "${namespace}" ] || fail "the holder names no account namespace"

init=$(api "${newnode}" POST "account/pair-init" \
    "{\"accountRootPublicKey\":\"${root_key}\",\"accountNamespace\":\"${namespace}\",\"namespaces\":[]}")
device=$(echo "${init}" | jq -r '.data.deviceId')
kem=$(echo "${init}" | jq -r '.data.kemPublicKey')
sign=$(echo "${init}" | jq -r '.data.signPublicKey')
statement=$(echo "${init}" | jq -r '.data.statement')
code=$(echo "${init}" | jq -r '.data.confirmationCode')

complete=$(api "${holder}" POST "account/pair-complete" \
    "{\"deviceId\":\"${device}\",\"kemPublicKey\":\"${kem}\",\"signPublicKey\":\"${sign}\",\"statement\":\"${statement}\",\"confirmationCode\":\"${code}\",\"applications\":[\"${app}\"]}")
[ "$(echo "${complete}" | jq -r '.data.keyDelivered')" = "true" ] \
    || fail "pair-complete delivered no key: ${complete}"

# The holder has always been able to say this, from the certificate it signed.
# Asserted so a registry that replaced the cache cannot quietly lose the scope.
api "${holder}" GET "account/devices" \
    | jq -e --arg d "${device}" --arg a "${app}" \
        'any(.devices[]; .deviceId == $d and (.applications | index($a)) != null)' >/dev/null \
    || fail "the holder does not report the phone as scoped to ${app}"

# The assertion this scenario exists for. The phone caches no certificate, so an
# empty scope here is the whole gap: it knows it is a device and not what for.
tries=45
while [ "${tries}" -gt 0 ]; do
    if api "${newnode}" GET "account/devices" \
        | jq -e --arg d "${device}" --arg a "${app}" \
            'any(.devices[]; .deviceId == $d and .isSelf and (.applications | index($a)) != null)' >/dev/null 2>&1; then
        break
    fi
    tries=$((tries - 1))
    sleep 2
done
[ "${tries}" -gt 0 ] || fail "the phone never learned its own scope from the account namespace"

# Corroborating, and already true before the registry: the holder's own device is
# bound in the account namespace by its genesis, so the binding scan finds it.
api "${newnode}" GET "account/devices" \
    | jq -e --arg d "${holder_device}" 'any(.devices[]; .deviceId == $d)' >/dev/null \
    || fail "the phone does not see the holder's device"

echo "device ${device} reads its own scope ${app} out of account namespace ${namespace}"
