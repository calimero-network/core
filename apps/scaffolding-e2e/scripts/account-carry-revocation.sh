#!/bin/sh
#
# Retire <lost-device> from <publisher>, holding nothing but <holder>'s recovery
# phrase, published into the ACCOUNT namespace, whose id no merobox step can name.
#
# args: [ <holder>, <publisher>, <lost-device> ]
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <holder> <publisher> <lost-device>" >&2
    exit 1
fi

# shellcheck source=apps/scaffolding-e2e/scripts/account-api.sh
. "$(dirname "$0")/account-api.sh"

holder="$1"
publisher="$2"
device="$3"

# <holder> must be STOPPED: `account export` opens the datastore and RocksDB's lock
# is exclusive. `sed -n 1p`, not `head -1`: head closes the pipe and merod panics.
phrase=$(offline_merod "${holder}" account export | sed -n '1p')
[ -n "${phrase}" ] || fail "the lost holder's recovery phrase came back empty"

# On a trap and not after the call: under `set -eu` a failing revoke-proof exits
# before any cleanup line, leaving the holder's phrase on disk.
phrase_file="data/${publisher}/recovery.txt"
trap 'rm -f "${phrase_file}"' EXIT
printf '%s\n' "${phrase}" > "${phrase_file}"
# The same split offline_merod makes: a binary-mode node reads the file where it
# lies, a container reads it under the home the bind mount put it at.
if [ -x "${MEROD_BIN}" ]; then home="data/${publisher}"; else home=/app/data; fi
proof=$(offline_merod "${publisher}" account revoke-proof \
    --device "${device}" --from "${home}/recovery.txt" | sed -n '1p')
[ -n "${proof}" ] || fail "minting the revocation proof produced nothing"

account_namespace=$(api "${publisher}" GET "identity" | jq -r '.data.accountNamespaceId // empty')
[ -n "${account_namespace}" ] || fail "${publisher} follows no account namespace"

revoked=$(api "${publisher}" POST "namespaces/${account_namespace}/account/revoke" \
    "$(jq -nc --arg d "${device}" --arg p "${proof}" '{deviceId:$d,proof:$p}')")
echo "${revoked}" | jq -e --arg ns "${account_namespace}" \
    'any(.data.revokedIn[]; .namespaceId == $ns)' >/dev/null \
    || fail "the revocation did not reach the account namespace: ${revoked}"

echo "${publisher} revoked ${device} in the account namespace from the phrase alone"
