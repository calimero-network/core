#!/bin/sh
#
# Assert, from <founder>'s own device listing, that <scoped-device> is bound in
# the account namespace and NOT in <namespace>: a founder carries only the
# siblings whose scope covers what the namespace it founded targets.
#
# The account namespace id is read off <holder>'s identity, which is the only
# place it is published.
#
# args: [ <founder>, <holder>, <namespace>, <scoped-device> ]
set -eu

if [ "$#" -ne 4 ]; then
    echo "usage: $0 <founder> <holder> <namespace> <scoped-device>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

founder="$1"
holder="$2"
namespace="$3"
scoped="$4"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

account_namespace=$(api "${holder}" GET "identity" | jq -r '.data.accountNamespaceId // empty')
[ -n "${account_namespace}" ] || fail "the holder names no account namespace"

listing=$(api "${founder}" GET "account/devices")
echo "${listing}" | jq -e --arg d "${scoped}" --arg ns "${account_namespace}" \
    'any(.devices[]; .deviceId == $d and (.namespaces | index($ns)) != null)' >/dev/null \
    || fail "the scoped device is not bound in the account namespace: ${listing}"
echo "${listing}" | jq -e --arg d "${scoped}" --arg ns "${namespace}" \
    'any(.devices[]; .deviceId == $d and (.namespaces | index($ns)) == null)' >/dev/null \
    || fail "the scoped device was carried into a namespace its scope does not cover"

echo "device ${scoped} stayed in the account namespace and out of ${namespace}"
