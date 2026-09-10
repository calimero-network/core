#!/bin/sh
#
# Retire <lost-device> from <publisher>, holding nothing but <holder>'s recovery
# phrase. The proof is minted where no account root lives, and published into the
# ACCOUNT namespace, whose id no merobox step can name.
#
# <holder> must be STOPPED: `account export` opens the datastore directly and
# RocksDB's lock is exclusive. `revoke-proof --from` opens no store at all, which
# is why it can run on a node that is up.
#
# args: [ <holder>, <publisher>, <lost-device> ]
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <holder> <publisher> <lost-device>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
publisher="$2"
device="$3"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# `sed -n 1p` and not `head -1`: head closes the pipe, merod panics printing the
# rest, and a panic in the log reads as a broken step rather than a read.
phrase=$(offline_merod "${holder}" account export | sed -n '1p')
[ -n "${phrase}" ] || fail "the lost holder's recovery phrase came back empty"

# On a trap and not after the call: under `set -eu` a failing revoke-proof exits
# before any cleanup line, leaving the holder's phrase on disk.
phrase_file="data/${publisher}/recovery.txt"
trap 'rm -f "${phrase_file}"' EXIT
printf '%s\n' "${phrase}" > "${phrase_file}"
proof=$(offline_merod "${publisher}" account revoke-proof \
    --device "${device}" --from "$(offline_home "${publisher}")/recovery.txt" | sed -n '1p')
[ -n "${proof}" ] || fail "minting the revocation proof produced nothing"

account_namespace=$(api "${publisher}" GET "identity" | jq -r '.data.accountNamespaceId // empty')
[ -n "${account_namespace}" ] || fail "${publisher} follows no account namespace"

revoked=$(api "${publisher}" POST "namespaces/${account_namespace}/account/revoke" \
    "$(jq -nc --arg d "${device}" --arg p "${proof}" '{deviceId:$d,proof:$p}')")
echo "${revoked}" | jq -e --arg ns "${account_namespace}" \
    'any(.data.revokedIn[]; .namespaceId == $ns)' >/dev/null \
    || fail "the revocation did not reach the account namespace: ${revoked}"

echo "${publisher} revoked ${device} in the account namespace from the phrase alone"
