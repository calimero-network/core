#!/bin/sh
#
# What `POST /admin-api/account/devices/:device_id/relink` refuses once the
# device has been revoked.
#
#     - name: Relink refuses a spent id
#       type: script
#       script: scripts/account-relink-refusal-revoked.sh
#       target: local
#       args: [ <holder>, <revoked-device-id> ]
#
# Asked of the HOLDER, which is the only node that can reach this refusal: it
# holds the certificate, so the repair gets past the lookup and lands on the
# tombstone. Run AFTER the revocation, and it reads the holder\'s own store,
# which its own revocation wrote before it answered.
#
# `403` and permanently so. The tombstone is per namespace but the id is spent
# everywhere, so a repair that quietly worked around a revocation would be
# repairing the wrong thing: enrolling the machine afresh mints a NEW device id,
# and that is the only way back.

set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <holder> <revoked-device-id>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
revoked_device="$2"

got=$(api_post_status "${holder}" "account/devices/${revoked_device}/relink" '{"applications":[]}')
if [ "${got}" != "403" ]; then
    echo "relink on ${holder} answered ${got} to a relink of a revoked device, expected 403" >&2
    exit 1
fi
echo "a relink of a revoked device refused with 403, as it must be"
