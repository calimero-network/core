#!/bin/sh
#
# Pair <new-node> onto <holder>'s account with only the account namespace id, and
# check both nodes agree on it and see the device bound there.
#
# With an application csv, the pairing is scoped to those applications and the
# new node must also read its OWN scope back out of the namespace. It holds no
# certificate cache, so the scope can only have reached it as replicated state.
#
# args: [ <holder>, <new-node>, <application-csv|-> ]
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <holder> <new-node> <application-csv|->" >&2
    exit 1
fi

# shellcheck source=apps/scaffolding-e2e/scripts/account-api.sh
. "$(dirname "$0")/account-api.sh"

holder="$1"
newnode="$2"
applications="$3"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# Deterministic from the root, so the holder names it before anything created it.
identity=$(api "${holder}" GET "identity")
root_key=$(echo "${identity}" | jq -r '.data.accountRootPublicKey')
namespace=$(echo "${identity}" | jq -r '.data.accountNamespaceId // empty')
holder_device=$(echo "${identity}" | jq -r '.data.deviceId')
[ -n "${namespace}" ] || fail "the holder names no account namespace before pairing"

if [ "${applications}" = "-" ]; then
    scope='[]'
else
    scope=$(echo "${applications}" | jq -Rc 'split(",")')
fi

pair_init "${newnode}" \
    "{\"accountRootPublicKey\":\"${root_key}\",\"accountNamespace\":\"${namespace}\",\"namespaces\":[]}"

complete=$(api "${holder}" POST "account/pair-complete" \
    "{\"deviceId\":\"${device}\",\"kemPublicKey\":\"${kem}\",\"signPublicKey\":\"${sign}\",\"statement\":\"${statement}\",\"confirmationCode\":\"${code}\",\"applications\":${scope}}")
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
        break
    fi
    tries=$((tries - 1))
    sleep 2
done
[ "${tries}" -gt 0 ] || fail "the device never resolved its own binding in the account namespace"

if [ "${applications}" = "-" ]; then
    echo "device ${device} follows account namespace ${namespace} on both nodes"
    exit 0
fi

# The holder has always been able to say this, from the certificate it signed.
# Asserted so a registry that replaced the cache cannot quietly lose the scope.
api "${holder}" GET "account/devices" \
    | jq -e --arg d "${device}" --argjson a "${scope}" \
        'any(.devices[]; .deviceId == $d and ($a - .applications) == [])' >/dev/null \
    || fail "the holder does not report the phone as scoped to ${applications}"

# The assertion the scoped run exists for. The phone caches no certificate, so
# an empty scope here is the whole gap: it knows it is a device and not what for.
tries=45
while [ "${tries}" -gt 0 ]; do
    if api "${newnode}" GET "account/devices" \
        | jq -e --arg d "${device}" --argjson a "${scope}" \
            'any(.devices[]; .deviceId == $d and .isSelf and ($a - .applications) == [])' >/dev/null 2>&1; then
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

echo "device ${device} reads its own scope ${applications} out of account namespace ${namespace}"
