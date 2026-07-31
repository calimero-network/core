#!/bin/sh
#
# Pair a second node onto an existing account — the full two-way exchange.
#
#     - name: Pair node-3 onto Alice's account
#       type: script
#       script: scripts/account-pair.sh
#       target: local
#       args: [ <holder-container>, <new-container>, <namespace-id> ]
#
# Two commands, not one, and the ordering is forced rather than stylistic: the
# new device cannot mint its DeviceId until it knows the account (the id is
# H(account ‖ nonce)), and the holder cannot certify that device until it knows
# the id and the two keys the new node minted. So:
#
#   1. pair-init on the NEW node, given the account's genesis  -> device id,
#      KEM key, signing key, a signature over them, and a confirmation code
#   2. pair-complete on the HOLDER, given those                -> link + the
#      scope key wrapped for the new device
#
# Both of step 2's inputs matter: the statement proves the keys came from the
# device that minted them, and the confirmation code proves the holder is
# certifying the keys it was actually read. A real operator carries the code by a
# different channel than the payload; this script carries both, so it can check
# that each is enforced but cannot reproduce the channel separation itself.
#
# Step 2 is what makes the device usable at all. Without it the link lands and
# the new device holds no key, so it can write nothing it can read back.
#
# The new node is deliberately NOT a namespace member. Its entire right to
# participate comes from being a device of the account.

set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <holder-container> <new-container> <namespace-id>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
newnode="$2"
namespace="$3"

root_key=$(cat "$(state_file "${namespace}" root-key)")
nonce=$(cat "$(state_file "${namespace}" nonce)")

init=$(api_post "${newnode}" "namespaces/${namespace}/account/pair-init" \
    "{\"accountRootKey\":\"${root_key}\",\"accountNonce\":\"${nonce}\"}")

device=$(echo "${init}" | jq -r '.data.deviceId')
kem=$(echo "${init}" | jq -r '.data.kemPublicKey')
sign=$(echo "${init}" | jq -r '.data.signPublicKey')
statement=$(echo "${init}" | jq -r '.data.statement')
init_code=$(echo "${init}" | jq -r '.data.confirmationCode')

if [ -z "${device}" ] || [ "${device}" = "null" ]; then
    echo "pair-init returned no deviceId: ${init}" >&2
    exit 1
fi

if [ -z "${statement}" ] || [ "${statement}" = "null" ]; then
    echo "pair-init returned no statement: ${init}" >&2
    exit 1
fi

# Stand in for the attacker before standing in for the operator: offer the
# holder somebody else's KEM key under this real device id. That is the
# substitution the statement exists to refuse, and refusing it is what stops the
# scope-key fan-out reaching a device the account holder never saw. A success
# here is a security regression, not a flaky step, so it fails the run.
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

complete=$(api_post "${holder}" "namespaces/${namespace}/account/pair-complete" \
    "{\"deviceId\":\"${device}\",\"kemPublicKey\":\"${kem}\",\"signPublicKey\":\"${sign}\",\"statement\":\"${statement}\",\"confirmationCode\":\"${init_code}\"}")

delivered=$(echo "${complete}" | jq -r '.data.keyDelivered')
paired_account=$(echo "${complete}" | jq -r '.data.accountId')
complete_code=$(echo "${complete}" | jq -r '.data.confirmationCode')

# The holder echoes the code back, so this equality is now enforced node-side as
# well — the assertion is kept because a silent change to the derivation would
# otherwise only show up as a refusal much later, in a scenario that has nothing
# to do with pairing.
if [ "${init_code}" != "${complete_code}" ]; then
    echo "confirmation codes differ: pair-init ${init_code} vs pair-complete ${complete_code}" >&2
    exit 1
fi

# A link without the key is not a working device — it can author but reads
# nothing. Fail loudly here rather than let the next write fail for a reason
# that looks unrelated.
if [ "${delivered}" != "true" ]; then
    echo "paired ${newnode} but the scope key was NOT delivered: ${complete}" >&2
    exit 1
fi

printf '%s\n' "${device}" > "$(state_file "${namespace}" paired-device)"

echo "paired ${newnode}: account=${paired_account} device=${device} code=${complete_code}"
