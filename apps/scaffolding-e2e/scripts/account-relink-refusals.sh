#!/bin/sh
#
# What `POST /admin-api/account/devices/:device_id/relink` refuses.
#
#     - name: Relink refuses the wrong device and the wrong machine
#       type: script
#       script: scripts/account-relink-refusals.sh
#       target: local
#       args: [ <holder>, <paired-node>, <paired-device-id> ]
#
# Run while the pairing STANDS. The wrong-machine refusal is only reachable
# there: a revoked device re-enrols under its own account root - the sync
# key-recovery path releases a tombstoned row - so a paired node asked for a
# repair after its revocation is a node holding its own account, and it answers
# `404` for a device it never certified rather than `403` for the machine.
#
# The spent id is the third refusal and needs the opposite state, so it lives in
# `account-relink-refusal-revoked.sh`.

set -eu

# 64 hex characters that decode cleanly and name no device of this account, so
# the refusal comes from the certificate lookup rather than the width check.
UNKNOWN_DEVICE="1111111111111111111111111111111111111111111111111111111111111111"

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <holder> <paired-node> <paired-device-id>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
pairednode="$2"
paired_device="$3"

expect_status() {
    _want="$1"
    _node="$2"
    _device="$3"
    _what="$4"
    _got=$(api_post_status "${_node}" "account/devices/${_device}/relink" '{"applications":[]}')
    if [ "${_got}" != "${_want}" ]; then
        echo "relink on ${_node} answered ${_got} to ${_what}, expected ${_want}" >&2
        exit 1
    fi
    echo "${_what} refused with ${_want}, as it must be"
}

# `404`, not `403`: the thing being addressed does not exist here. Only a device
# this node paired, or whose link it has folded, carries the certificate a relink
# re-publishes, and that cannot be rebuilt from folded state.
expect_status 404 "${holder}" "${UNKNOWN_DEVICE}" "a device this node holds no certificate for"

# The right request at the wrong machine. This node's device belongs to an
# account its own root does not own - it adopted one by pairing - so it holds no
# root that could have signed the certificate it would re-publish. A paired
# device cannot repair further devices, and no retry here changes that.
expect_status 403 "${pairednode}" "${paired_device}" "a repair run on a paired device rather than the holder"

echo "both refusals a live pairing can raise answered their own status"
