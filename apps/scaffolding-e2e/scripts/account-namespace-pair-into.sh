#!/bin/sh
#
# Pair <new-node> onto <holder>'s account, following the holder's ACCOUNT
# NAMESPACE as well as any project namespaces named, and scoped to the
# applications named. merobox's account_pair step has no field for the account
# namespace, and a device that does not follow it never folds the registry.
#
# Proves both sides agree on the id, that the holder keeps it out of its project
# listing, and that it bound the device there - then returns only once the new
# node has folded what the holder wrote.
#
# args: [ <holder>, <new-node>, <namespace-csv|->, <application-csv|-> ]
set -eu

if [ "$#" -ne 4 ]; then
    echo "usage: $0 <holder> <new-node> <namespace-csv|-> <application-csv|->" >&2
    exit 1
fi

# shellcheck source=apps/scaffolding-e2e/scripts/account-api.sh
. "$(dirname "$0")/account-api.sh"

holder="$1"
newnode="$2"
namespaces="$3"
applications="$4"

# `-` for none, so an empty positional cannot be mistaken for a missing one.
json_array() {
    if [ "$1" = "-" ]; then
        echo '[]'
    else
        echo "$1" | jq -Rc 'split(",")'
    fi
}

scope=$(json_array "${applications}")

# Deterministic from the root, so the holder names it before anything created it.
identity=$(api "${holder}" GET "identity")
root_key=$(echo "${identity}" | jq -r '.data.accountRootPublicKey')
account_namespace=$(echo "${identity}" | jq -r '.data.accountNamespaceId // empty')
[ -n "${account_namespace}" ] || fail "the holder names no account namespace before pairing"

pair_init "${newnode}" \
    "$(jq -nc --arg root "${root_key}" --arg ns "${account_namespace}" \
        --argjson namespaces "$(json_array "${namespaces}")" \
        '{accountRootPublicKey:$root,accountNamespace:$ns,namespaces:$namespaces}')"

complete=$(api "${holder}" POST "account/pair-complete" \
    "$(jq -nc --arg d "${device}" --arg kem "${kem}" --arg sign "${sign}" \
        --arg statement "${statement}" --arg code "${code}" --argjson a "${scope}" \
        '{deviceId:$d,kemPublicKey:$kem,signPublicKey:$sign,statement:$statement,confirmationCode:$code,applications:$a}')")
[ "$(echo "${complete}" | jq -r '.data.keyDelivered')" = "true" ] \
    || fail "pair-complete delivered no key: ${complete}"

# Both sides name the same namespace.
recorded=$(api "${newnode}" GET "identity" | jq -r '.data.accountNamespaceId // empty')
[ "${recorded}" = "${account_namespace}" ] \
    || fail "the device recorded '${recorded}', the holder names '${account_namespace}'"

# A project listing never shows it.
api "${holder}" GET "namespaces" \
    | jq -e --arg ns "${account_namespace}" 'all(.data[]; .namespaceId != $ns)' >/dev/null \
    || fail "the holder lists the account namespace as a project"

# The holder bound the device there, and - from the certificate it signed, so a
# registry that replaced the cache cannot quietly lose it - with the scope named.
api "${holder}" GET "account/devices" \
    | jq -e --arg d "${device}" --arg ns "${account_namespace}" --argjson a "${scope}" \
        'any(.devices[]; .deviceId == $d and (.namespaces | index($ns)) != null
             and ($a - .applications) == [])' >/dev/null \
    || fail "the holder does not show the device bound in the account namespace, scoped to ${applications}"

# The barrier. A scope can only have come from the replicated registry, so
# reading its own scope back proves this node folded the LAST op the holder
# published there - and therefore every earlier one. With no scope named there
# is no such field, so the weaker "my own link is here" is all there is.
tries=45
while [ "${tries}" -gt 0 ]; do
    listing=$(api "${newnode}" GET "account/devices" 2>/dev/null || true)
    if echo "${listing}" | jq -e --arg d "${device}" --arg ns "${account_namespace}" \
        --argjson want "${scope}" \
        'any(.devices[]; .deviceId == $d and .isSelf
             and (if ($want | length) == 0
                  then (.namespaces | index($ns)) != null
                  else .applications == $want end))' \
        >/dev/null 2>&1; then
        break
    fi
    tries=$((tries - 1))
    sleep 2
done
[ "${tries}" -gt 0 ] || fail "the device never folded the account namespace"

# Corroborating, and already true before the registry: the holder's own device is
# bound in the account namespace by its genesis, so the binding scan finds it.
# Only behind a scope: that barrier proves the fold reached the holder's own
# statement, while the unscoped one proves no more than this device's own link.
# Read after the pairing: a holder that took part in nothing has no device to
# name until pair-complete enrols one and records it.
if [ "${applications}" != "-" ]; then
    holder_device=$(api "${holder}" GET "identity" | jq -r '.data.deviceId // empty')
    [ -n "${holder_device}" ] || fail "the holder names no device of its own"
    api "${newnode}" GET "account/devices" \
        | jq -e --arg d "${holder_device}" 'any(.devices[]; .deviceId == $d)' >/dev/null \
        || fail "the phone does not see the holder's device"
fi

echo "device ${device} follows account namespace ${account_namespace}, scope ${applications}"
