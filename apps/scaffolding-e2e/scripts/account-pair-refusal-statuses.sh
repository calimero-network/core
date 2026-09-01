#!/bin/sh
#
# The two `POST /admin-api/account/pair-complete` refusals that need a payload
# with one field substituted.
#
#     args: [ <holder>, <new-node>, <namespace-id>, <root-key> ]
#
# Still a script because merobox's `account_pair` derives the statement and the
# confirmation code from its own `pair-init`, so no scenario can hand it a bad
# one. The refusals that need only a different request - an unreachable scope, a
# device this node never certified - are native `expect_status` steps.
#
# Asserted by status, not by failure: a 200 is a security regression and a 500
# is the regression this mapping prevents.

set -eu

if [ "$#" -ne 4 ]; then
    echo "usage: $0 <holder> <new-node> <namespace-id> <root-key>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
newnode="$2"
namespace="$3"
root_key="$4"

init=$(api "${newnode}" POST "account/pair-init" \
    "{\"accountRootPublicKey\":\"${root_key}\",\"namespaces\":[\"${namespace}\"]}")

device=$(echo "${init}" | jq -r '.data.deviceId')
kem=$(echo "${init}" | jq -r '.data.kemPublicKey')
sign=$(echo "${init}" | jq -r '.data.signPublicKey')
statement=$(echo "${init}" | jq -r '.data.statement')
code=$(echo "${init}" | jq -r '.data.confirmationCode')

for value in "${device}" "${kem}" "${sign}" "${statement}" "${code}"; do
    if [ -z "${value}" ] || [ "${value}" = "null" ]; then
        echo "pair-init returned an incomplete payload: ${init}" >&2
        exit 1
    fi
done

# A `pair-complete` body over the minted material, with one thing substituted.
offer() {
    _statement="$1"
    _code="$2"
    printf '{"deviceId":"%s","kemPublicKey":"%s","signPublicKey":"%s","statement":"%s","confirmationCode":"%s","applications":[]}' \
        "${device}" "${kem}" "${sign}" "${_statement}" "${_code}"
}

# `refuse <status> <what> <body>` - the holder's `pair-complete` must answer
# `<status>` to `<body>`.
refuse() {
    expect_status "$1" "${holder}" "account/pair-complete" "$3" "$2"
}

# Still 128 hex characters, so it decodes and reaches the signature check rather
# than the width validator - the refusal under test is the verification failing,
# not the field being malformed.
tampered="0${statement#?}"
if [ "${tampered}" = "${statement}" ]; then
    tampered="1${statement#?}"
fi
refuse 400 "a tampered statement" "$(offer "${tampered}" "${code}")"

# The gate that stands between the account and a WHOLESALE substitution: one that
# replaces both keys and re-signs, so the statement verifies cleanly and only the
# code - which arrives from the other device by a channel the attacker does not
# control - disagrees.
refuse 400 "a mismatched confirmation code" \
    "$(offer "${statement}" "DEAD-BEEF-DEAD-BEEF")"

echo "both substituted-payload refusals answered their own status"
