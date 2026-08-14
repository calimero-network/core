#!/bin/sh
#
# The two substitutions `pair-complete` must refuse, driven as an attacker would.
#
#     - name: Pairing refuses substituted key material
#       type: script
#       script: scripts/account-pair-refusals.sh
#       target: local
#       args: [ <holder>, <new-node>, <namespace-id>, <root-key> ]
#
# The root key comes from the `account_create` step's `accountRootKey` output —
# passed in rather than read from a file, because the step exports it and nothing
# writes the temp files any more. There is no nonce: the account genesis carries
# none, and `pair-init` needs only the root key.
#
# Run this BEFORE the `account_pair` step does the real pairing: it performs its
# own `pair-init` on the new node and then offers the holder two deliberately
# wrong payloads, asserting both are refused. It never completes the pairing.
#
# Why a script and not a workflow step. merobox's `account_pair` runs both halves
# of the sanctioned exchange and passes what `pair-init` minted through verbatim —
# which is exactly right for the happy path and exactly why it cannot express
# these: an attacker's whole move is to send something the minting device did not
# produce. Crafting a hostile payload is script territory. (Were merobox to expose
# `pair_device_init` and `pair_device_complete` as separate steps, these two cases
# could move into the scenario as `expected_failure` steps; the bindings exist in
# calimero-client-py 0.6.20, only the step split is missing.)
#
# A success in either case is a security regression, not a flaky step, so both
# fail the run.

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

init=$(api_post "${newnode}" "namespaces/${namespace}/account/pair-init" \
    "{\"accountRootKey\":\"${root_key}\"}")

device=$(echo "${init}" | jq -r '.data.deviceId')
kem=$(echo "${init}" | jq -r '.data.kemPublicKey')
sign=$(echo "${init}" | jq -r '.data.signPublicKey')
statement=$(echo "${init}" | jq -r '.data.statement')
init_code=$(echo "${init}" | jq -r '.data.confirmationCode')

for value in "${device}" "${statement}" "${init_code}"; do
    if [ -z "${value}" ] || [ "${value}" = "null" ]; then
        echo "pair-init returned an incomplete payload: ${init}" >&2
        exit 1
    fi
done

# Stand in for the attacker: offer the holder somebody else's KEM key under this
# real device id and real statement. That is the substitution the statement exists
# to refuse, and refusing it is what stops the scope-key fan-out reaching a device
# the account holder never saw.
forged_kem="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
if forged=$(api_post "${holder}" "namespaces/${namespace}/account/pair-complete" \
    "{\"deviceId\":\"${device}\",\"kemPublicKey\":\"${forged_kem}\",\"signPublicKey\":\"${sign}\",\"statement\":\"${statement}\",\"confirmationCode\":\"${init_code}\"}" 2>/dev/null) \
    && [ "$(echo "${forged}" | jq -r '.data.accountId // empty')" != "" ]; then
    echo "pair-complete CERTIFIED substituted key material: ${forged}" >&2
    exit 1
fi
echo "substituted KEM key refused, as it must be"

# The other gate, on the honest payload: a code that does not describe this key
# material is refused. This is what stands between the account and a WHOLESALE
# substitution — one that replaces both keys and re-signs, so the statement above
# verifies cleanly and only the code disagrees.
if wrong=$(api_post "${holder}" "namespaces/${namespace}/account/pair-complete" \
    "{\"deviceId\":\"${device}\",\"kemPublicKey\":\"${kem}\",\"signPublicKey\":\"${sign}\",\"statement\":\"${statement}\",\"confirmationCode\":\"DEAD-BEEF-DEAD-BEEF\"}" 2>/dev/null) \
    && [ "$(echo "${wrong}" | jq -r '.data.accountId // empty')" != "" ]; then
    echo "pair-complete CERTIFIED with a mismatched confirmation code: ${wrong}" >&2
    exit 1
fi
echo "mismatched confirmation code refused, as it must be"

echo "both substitutions refused; the real pairing is the account_pair step"
