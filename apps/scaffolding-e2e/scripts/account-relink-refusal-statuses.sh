#!/bin/sh
#
# What `POST /admin-api/account/devices/:device_id/relink` refuses.
#
#     - name: Relink refuses the wrong device, the wrong machine and a spent id
#       type: script
#       script: scripts/account-relink-refusal-statuses.sh
#       target: local
#       args: [ <holder>, <paired-node>, <paired-device-id> ]
#
# Run AFTER the device has been revoked, because that is the only ordering in
# which all three are reachable at once - and the first two are unaffected by the
# revocation, so nothing is weakened by asking for them here.
#
# The three are the whole distinction a caller can act on: the device does not
# exist here, you are at the wrong machine, and this id is spent for good.

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

# `403` and permanently so. The tombstone is per namespace but the id is spent
# everywhere, so a repair that quietly worked around a revocation would be
# repairing the wrong thing: enrolling the machine afresh mints a NEW device id,
# and that is the only way back.
expect_status 403 "${holder}" "${paired_device}" "a relink of a revoked device"

echo "all three relink refusals answered their own status"
