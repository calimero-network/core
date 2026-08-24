#!/bin/sh
#
# Drive a delegated write end to end: a member with no node authorises a relay,
# the relay runs the method, and the result is attributed to the member.
#
#     - name: A member with no node writes through the relay
#       type: script
#       script: scripts/delegated-intent.sh
#       target: local
#       args: [ <relay-container>, <context-id>, <group-id>, <nonce> ]
#
# Why a script and not workflow steps. The author here is deliberately NOT a
# node: it is a device holding only a signing key, which is the case delegated
# authorship exists for. Nothing in merobox can hold a key and sign with it, and
# nothing should — so the signing happens through `merod`'s offline account
# commands, which open no store and contact no node. They run inside a container
# because the merod image ships that binary and a `target: local` script has no
# meroctl (see account-api.sh's header).
#
# The account root used here is a FIXED test phrase, so the account is
# deterministic and this script can add it as a member before certifying a device
# for it. It owns nothing anywhere else.

set -eu

if [ "$#" -ne 4 ]; then
    echo "usage: $0 <relay-container> <context-id> <group-id> <nonce>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

relay="$1"
context="$2"
group="$3"
nonce="$4"

# Not a secret: it owns nothing, and it is here so the author's account is the
# same on every run. Any real holder keeps their own phrase off the machine that
# runs the relay.
phrase="legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title"

run_in_relay() {
    docker exec "${relay}" "$@"
}

echo "--- provisioning an author device that holds no node"

# `--generate` mints the device and certifies it in one step, so the secret
# exists only in this output. `--from` means no store is opened, which is why
# this is safe to run against a container whose node is live.
printf '%s\n' "${phrase}" > /tmp/delegated-author-phrase
docker cp /tmp/delegated-author-phrase "${relay}:/tmp/author-phrase" >/dev/null

cert_out=$(run_in_relay merod --node "${relay}" account sign-cert \
    --generate --from /tmp/author-phrase) || {
    echo "sign-cert failed: ${cert_out}" >&2
    exit 1
}

credential=$(printf '%s\n' "${cert_out}" | head -1)
author_account=$(printf '%s\n' "${cert_out}" | sed -n 's/^Account: *//p')
author_secret=$(printf '%s\n' "${cert_out}" | sed -n 's/^Secret: *//p')

for v in "${credential}" "${author_account}" "${author_secret}"; do
    if [ -z "${v}" ]; then
        echo "sign-cert output was incomplete:" >&2
        printf '%s\n' "${cert_out}" >&2
        exit 1
    fi
done
echo "author account ${author_account}, device certified"

echo "--- the author's ACCOUNT becomes a member; its device joins nothing"

# By account, not by key — the device is in no group's binding rows and never
# will be, which is exactly the case a certificate exists to cover.
api_post "${relay}" "groups/${group}/members" \
    "{\"members\":[{\"identity\":\"${author_account}\",\"role\":\"Member\"}]}" >/dev/null
echo "author added as a member"

echo "--- the relay is granted authorship, which it does not have by default"

relay_identity=$(api_get "${relay}" "identity")
relay_account=$(printf '%s' "${relay_identity}" | jq -r '.data.accountId')
if [ -z "${relay_account}" ] || [ "${relay_account}" = "null" ]; then
    echo "could not read the relay's account: ${relay_identity}" >&2
    exit 1
fi

# Refused BEFORE the grant, which is the half that proves the grant is
# load-bearing rather than incidental. DAR-11: this must be a clean refusal at
# the API, never a published delta peers would drop.
args='{"key":"delegated","value":"from-a-member-with-no-node"}'
warrant=$(run_in_relay merod --node "${relay}" account warrant \
    --context "${context}" --method set --args "${args}" \
    --executor "${relay_account}" --nonce "${nonce}" \
    --device-secret "${author_secret}" --credential "${credential}") || {
    echo "minting the warrant failed" >&2
    exit 1
}

body="{\"method\":\"set\",\"argsJson\":${args},\"warrant\":\"${warrant}\",\"authorProof\":\"${credential}\"}"

# Asserting the STATUS, not merely that it failed. "Any error counts" would be
# satisfied by a node that crashed on the request, which is the opposite of the
# clean refusal DAR-11 asks for — and would let a real regression pass here.
early=$(api_post_status "${relay}" "contexts/${context}/intents" "${body}")
case "${early}" in
    *403*) echo "refused with 403 before the grant, as it must be" ;;
    *2[0-9][0-9]*)
        echo "the relay performed an intent WITHOUT an authorship grant: ${early}" >&2
        exit 1
        ;;
    *)
        echo "refused, but not with 403 — a clean refusal is part of the contract" >&2
        echo "  got: ${early}" >&2
        exit 1
        ;;
esac

# CAN_AUTHOR_ON_BEHALF is bit 9, so 512 on its own — granting authorship and
# nothing else, which is the posture the capability exists to make possible.
api_put "${relay}" "groups/${group}/members/${relay_account}/capabilities" \
    '{"capabilities":512}' >/dev/null
echo "relay granted CAN_AUTHOR_ON_BEHALF"

echo "--- the same warrant, now spendable"

accepted=$(api_post "${relay}" "contexts/${context}/intents" "${body}") || {
    echo "the intent was refused after the grant was made" >&2
    exit 1
}
echo "intent performed: ${accepted}"

echo "--- and it is single-use"

# The nonce is spent. A relay re-presenting the same authorization is the attack
# the ledger exists for, and it must fail even though the warrant is still
# perfectly valid — replay is not forgery.
# No status pinned here, unlike the check above, and the asymmetry is deliberate.
# The pre-grant refusal is a decision this endpoint makes itself, so it owes a
# 403. A replay is caught by the nonce ledger inside the apply, where 5xx is a
# legitimate answer — the node may also be unable to resolve authority yet. What
# must hold is only that it is not accepted.
replay=$(api_post_status "${relay}" "contexts/${context}/intents" "${body}")
case "${replay}" in
    *2[0-9][0-9]*)
        echo "a spent warrant was accepted a second time: ${replay}" >&2
        exit 1
        ;;
    *) echo "the replay was refused: ${replay}" ;;
esac

echo "delegated write completed: author ${author_account} via relay ${relay_account}"
