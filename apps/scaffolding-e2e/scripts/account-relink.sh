#!/bin/sh
#
# `POST /admin-api/account/devices/:device_id/relink` - widen a device's scope
# and repair its bindings, without the device being present.
#
#     - name: Widen the phone's scope to app B
#       type: script
#       script: scripts/account-relink.sh
#       target: local
#       args: [ <holder>, <device-id>, <add-applications>, <linked>, <already-bound>, <scope> ]
#
# Every list is comma-separated, and `-` skips that check (or, for
# `<add-applications>`, sends none, which is the repair-only request).
#
# `<linked>` and `<already-bound>` are what makes this endpoint's answer worth
# having: publication is per-DAG, so "it worked" is a set of namespaces rather
# than a boolean, and a namespace that was skipped has to say why. A relink that
# re-published where the device was already bound would be re-endorsing a live
# binding on every call.

set -eu

if [ "$#" -ne 6 ]; then
    echo "usage: $0 <holder> <device-id> <add-applications> <linked> <already-bound> <scope>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
device="$2"
add_applications="$3"
want_linked="$4"
want_already_bound="$5"
want_scope="$6"

canonical() {
    printf '%s' "$1" | tr ',' '\n' | grep -v '^$' | sort | tr '\n' ' '
}

json_array() {
    if [ -z "$1" ] || [ "$1" = "-" ]; then
        echo '[]'
        return 0
    fi
    printf '%s' "$1" | jq -Rc 'split(",") | map(select(length > 0))'
}

relinked=$(api_post "${holder}" "account/devices/${device}/relink" \
    "{\"applications\":$(json_array "${add_applications}")}")

if [ "$(echo "${relinked}" | jq -r '.data.deviceId')" != "${device}" ]; then
    echo "relink answered for a device nobody named: ${relinked}" >&2
    exit 1
fi

if [ "${want_scope}" != "-" ]; then
    have=$(canonical "$(echo "${relinked}" | jq -r '.data.applications | join(",")')")
    want=$(canonical "${want_scope}")
    if [ "${have}" != "${want}" ]; then
        echo "${device} is now scoped to [${have}], expected exactly [${want}]" >&2
        exit 1
    fi
    echo "${device}'s stored scope is now exactly [${want}]"
fi

if [ "${want_linked}" != "-" ]; then
    for namespace in $(printf '%s' "${want_linked}" | tr ',' ' '); do
        entry=$(echo "${relinked}" | jq -c --arg n "${namespace}" \
            '.data.linkedIn[] | select(.namespaceId == $n)')
        if [ -z "${entry}" ]; then
            echo "relink published nothing into ${namespace}: ${relinked}" >&2
            exit 1
        fi
        # A link with no key leaves the device authorized and unable to read
        # until its own sync pull re-requests the key, so the two are asserted
        # together rather than treating publication as the whole outcome.
        if [ "$(echo "${entry}" | jq -r '.keyDelivered')" != "true" ]; then
            echo "relink published into ${namespace} without delivering the scope key: ${entry}" >&2
            exit 1
        fi
        echo "relink linked ${device} into ${namespace} and delivered the key"
    done
fi

if [ "${want_already_bound}" != "-" ]; then
    for namespace in $(printf '%s' "${want_already_bound}" | tr ',' ' '); do
        reason=$(echo "${relinked}" | jq -r --arg n "${namespace}" \
            '.data.skipped[] | select(.namespaceId == $n) | .reason')
        if [ "${reason}" != "alreadyBound" ]; then
            echo "relink reported ${namespace} as '${reason}', expected 'alreadyBound'" >&2
            exit 1
        fi
        echo "relink left ${namespace} alone: ${device} was already bound there"
    done
fi
