#!/bin/sh
#
# One row of `GET /admin-api/account/devices`, asserted.
#
#     - name: The paired device reaches app A's namespace and not app B's
#       type: script
#       script: scripts/account-device-scope.sh
#       target: local
#       args: [ <holder>, <device-id>, <applications>, <namespaces>, <not-namespaces> ]
#
# Every list is comma-separated, and `-` skips that check.
#
# `<applications>` is an EXACT set, because that is the claim: a device scoped to
# one application must carry that one and nothing else. Empty is not expressible
# and does not need to be - an empty stored scope means "every application" here,
# which is a different assertion from "scoped to exactly these".
#
# `<namespaces>` and `<not-namespaces>` are membership, not a set, because the
# bound set grows over a scenario's life: a namespace gained later binds the
# device on its own, so pinning the whole set would fail on the feature working.
# The absence list is what carries the weight - it is the only way to say that a
# scope actually NARROWED rather than merely reached what it was asked for.

set -eu

if [ "$#" -ne 5 ]; then
    echo "usage: $0 <holder> <device-id> <applications> <namespaces> <not-namespaces>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

holder="$1"
device="$2"
want_applications="$3"
want_namespaces="$4"
unwanted_namespaces="$5"

# A comma-separated list as sorted, space-separated words, so two lists written
# in different orders compare equal.
canonical() {
    printf '%s' "$1" | tr ',' '\n' | grep -v '^$' | sort | tr '\n' ' '
}

devices=$(api_get "${holder}" "account/devices")
row=$(echo "${devices}" | jq -c --arg d "${device}" '.devices[] | select(.deviceId == $d)')
if [ -z "${row}" ]; then
    echo "${holder} lists no device ${device}: ${devices}" >&2
    exit 1
fi

# This is somebody else's installation as far as the holder is concerned, and a
# row that claimed otherwise would mean the holder had adopted the device's
# identity as its own.
if [ "$(echo "${row}" | jq -r '.isSelf')" != "false" ]; then
    echo "${holder} reports ${device} as its own device: ${row}" >&2
    exit 1
fi
if [ "$(echo "${row}" | jq -r '.revoked')" != "false" ]; then
    echo "${device} is already revoked on ${holder}: ${row}" >&2
    exit 1
fi

if [ "${want_applications}" != "-" ]; then
    have=$(canonical "$(echo "${row}" | jq -r '.applications | join(",")')")
    want=$(canonical "${want_applications}")
    if [ "${have}" != "${want}" ]; then
        echo "${device} is scoped to [${have}], expected exactly [${want}]" >&2
        exit 1
    fi
    echo "${device} is scoped to exactly [${want}], as it must be"
fi

bound=$(echo "${row}" | jq -r '.namespaces[]')

if [ "${want_namespaces}" != "-" ]; then
    for namespace in $(printf '%s' "${want_namespaces}" | tr ',' ' '); do
        if ! echo "${bound}" | grep -qx "${namespace}"; then
            echo "${device} holds no binding in ${namespace}; bound in: $(echo "${bound}" | tr '\n' ' ')" >&2
            exit 1
        fi
        echo "${device} is bound in ${namespace}"
    done
fi

if [ "${unwanted_namespaces}" != "-" ]; then
    for namespace in $(printf '%s' "${unwanted_namespaces}" | tr ',' ' '); do
        if echo "${bound}" | grep -qx "${namespace}"; then
            echo "${device} is bound in ${namespace}, which its application scope does not cover" >&2
            exit 1
        fi
        echo "${device} is absent from ${namespace}, as its scope requires"
    done
fi
