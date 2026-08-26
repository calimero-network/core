#!/bin/sh
#
# What `POST /admin-api/account/pair-complete` refuses, and with which status.
#
#     - name: Pairing refuses a bad payload and an unreachable scope
#       type: script
#       script: scripts/account-pair-refusal-statuses.sh
#       target: local
#       args: [ <holder>, <new-node>, <namespace-id>, <root-key>, <unbound-application> ]
#
# `<unbound-application>` must be an application the holder serves in no
# namespace YET, which is what makes the scope resolve to nothing here. An
# application whose namespace comes later does exactly as well as one that never
# gets one, so a scenario can spend its second application on this and then go
# on to use it.
#
# Asserted by STATUS, not by failure. Three of these used to be one `500` apiece,
# which told a client nothing it could act on: retrying a mistyped confirmation
# code is pointless, retrying a scope this node does not yet take part in is
# exactly right, and neither is a broken node. A `200` is a security regression
# and a `500` is the regression this mapping exists to prevent, so both fail the
# run.
#
# Runs BEFORE the real pairing. It performs its own `pair-init` - idempotent, so
# the pairing that follows mints the same device and reads out the same code,
# even when it names more namespaces than this one did - and never completes one.
#
# Two refusals this cannot drive, deliberately:
#   * `PairingNoScopeKey` (409). This node holds a scope key in every namespace it
#     takes part in, and a namespace it does not take part in fails the identity
#     check first - so the two 409s cannot both be reached from one node's state.
#   * `PairingNotTheAccountHolder` (403) on this route. Reaching it needs a
#     statement over the account THIS node's own root owns, and a node never
#     publishes that root once it has adopted another account, so no caller can
#     produce one. The same refusal is reachable on `relink`, which is where
#     `account-relink-refusal-statuses.sh` drives it.

set -eu

if [ "$#" -ne 5 ]; then
    echo "usage: $0 <holder> <new-node> <namespace-id> <root-key> <unbound-application>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
newnode="$2"
namespace="$3"
root_key="$4"
unbound_application="$5"

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
    _applications="$3"
    printf '{"deviceId":"%s","kemPublicKey":"%s","signPublicKey":"%s","statement":"%s","confirmationCode":"%s","applications":%s}' \
        "${device}" "${kem}" "${sign}" "${_statement}" "${_code}" "${_applications}"
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
refuse 400 "a tampered statement" "$(offer "${tampered}" "${code}" '[]')"

# The gate that stands between the account and a WHOLESALE substitution: one that
# replaces both keys and re-signs, so the statement verifies cleanly and only the
# code - which arrives from the other device by a channel the attacker does not
# control - disagrees.
refuse 400 "a mismatched confirmation code" \
    "$(offer "${statement}" "DEAD-BEEF-DEAD-BEEF" '[]')"

# `409`, not `400`: the payload is perfect and the identical call works once this
# node takes part in a namespace targeting that application. Checked before the
# payload is looked at, so this says the SCOPE was refused rather than the keys.
refuse 409 "a scope this node signs nowhere in" \
    "$(offer "${statement}" "${code}" "[\"${unbound_application}\"]")"

echo "every refusal answered its own status; the real pairing is the account-pair.sh step"
