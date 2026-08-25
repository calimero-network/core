#!/bin/sh
#
# The account-level pairing exchange, over the two routes that replaced the
# namespace-scoped ones.
#
#     - name: Pair the phone onto Alice's account
#       type: script
#       script: scripts/account-pair.sh
#       target: local
#       args: [ <new-node>, <holder>, <namespace-ids>, <root-key>, <application-ids> ]
#
# `<namespace-ids>` and `<application-ids>` are comma-separated; `-` for the
# applications means every application, which is what a caller who names none
# asks for. Both lists are the point of these routes: the namespace set decides
# what the new device LISTENS on, the application set decides where the holder
# PUBLISHES the link, and the two are deliberately independent.
#
# Why a script rather than merobox's `account_pair` step. That step drives
# `namespaces/:id/account/pair-init` and `.../pair-complete` - one namespace from
# the path, no application scope - which is exactly the shape these routes exist
# to replace. It cannot express a set or a scope, so nothing it does would
# exercise what is under test here.
#
# Exports nothing: a `script` step cannot, and it does not need to. Every value
# minted here is readable afterwards from the new device's own
# `GET /admin-api/identity`, which a `node_identity` step captures.

set -eu

if [ "$#" -ne 5 ]; then
    echo "usage: $0 <new-node> <holder> <namespace-ids> <root-key> <application-ids>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

newnode="$1"
holder="$2"
namespaces="$3"
root_key="$4"
applications="$5"

# A comma-separated list as a JSON array of strings. `-` is the empty list,
# which is how "every application" is spelled on the wire.
json_array() {
    if [ -z "$1" ] || [ "$1" = "-" ]; then
        echo '[]'
        return 0
    fi
    printf '%s' "$1" | jq -Rc 'split(",") | map(select(length > 0))'
}

init=$(api_post "${newnode}" "account/pair-init" \
    "{\"accountRootPublicKey\":\"${root_key}\",\"namespaces\":$(json_array "${namespaces}")}")

account=$(echo "${init}" | jq -r '.data.accountId')
device=$(echo "${init}" | jq -r '.data.deviceId')
kem=$(echo "${init}" | jq -r '.data.kemPublicKey')
sign=$(echo "${init}" | jq -r '.data.signPublicKey')
statement=$(echo "${init}" | jq -r '.data.statement')
code=$(echo "${init}" | jq -r '.data.confirmationCode')

for value in "${account}" "${device}" "${kem}" "${sign}" "${statement}" "${code}"; do
    if [ -z "${value}" ] || [ "${value}" = "null" ]; then
        echo "pair-init returned an incomplete payload: ${init}" >&2
        exit 1
    fi
done
echo "minted device ${device} for account ${account} across ${namespaces}"

complete=$(api_post "${holder}" "account/pair-complete" \
    "{\"deviceId\":\"${device}\",\"kemPublicKey\":\"${kem}\",\"signPublicKey\":\"${sign}\",\"statement\":\"${statement}\",\"confirmationCode\":\"${code}\",\"applications\":$(json_array "${applications}")}")

# The holder certifies into the account its OWN root owns, so an account that
# disagrees with the one the device minted under means the two halves ran against
# different accounts and the certificate names a device nothing will admit.
certified_account=$(echo "${complete}" | jq -r '.data.accountId')
certified_device=$(echo "${complete}" | jq -r '.data.deviceId')
if [ "${certified_account}" != "${account}" ] || [ "${certified_device}" != "${device}" ]; then
    echo "pair-complete certified ${certified_device}/${certified_account}, not ${device}/${account}" >&2
    exit 1
fi

# The code is echoed back so the operator can see what the certificate names,
# and it has to be the one the device read out: a different one would mean the
# holder certified key material the person never compared.
if [ "$(echo "${complete}" | jq -r '.data.confirmationCode')" != "${code}" ]; then
    echo "pair-complete echoed a confirmation code the pairing device never minted: ${complete}" >&2
    exit 1
fi

# The link is what confers authority and the key is what makes the device able to
# read, so a pairing that delivered no key leaves a device that can write and see
# nothing. Asserted rather than reported, because everything downstream of here
# reads state the device could only reach with it.
if [ "$(echo "${complete}" | jq -r '.data.keyDelivered')" != "true" ]; then
    echo "pair-complete did not deliver a scope key: ${complete}" >&2
    exit 1
fi

# The certificate itself, borsh-hex. A thin client cannot read it off the DAG, so
# an empty one is a device that cannot present itself anywhere.
if [ -z "$(echo "${complete}" | jq -r '.data.credential // empty')" ]; then
    echo "pair-complete minted no credential: ${complete}" >&2
    exit 1
fi

echo "certified ${device} into ${account}, scoped to ${applications}, scope key delivered"
