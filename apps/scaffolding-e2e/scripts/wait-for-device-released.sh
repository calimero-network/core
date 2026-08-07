#!/bin/sh
#
# Block until a node has folded a device revocation, i.e. until it reports no
# device of its own in the namespace.
#
#     - name: Wait until node-2 has folded the revocation
#       type: script
#       script: scripts/wait-for-device-released.sh
#       target: local
#       args: [ <node>, <namespace-id>, <timeout-seconds> ]
#
# GET /admin-api/namespaces/<ns>/account is the same read merobox's `account_show`
# step performs, and `deviceId: null` is a real answer meaning "this node holds no
# device here" — which is exactly the post-condition of a revocation the node has
# applied. Polling it turns "has the tombstone landed?" into a question with an
# answer, instead of a guess about how long gossip takes.
#
# Why this exists. The revocation is a GOVERNANCE op, so `wait_for_sync` — which
# compares context state — does not gate it and returns in milliseconds. The
# scenario therefore slept a flat 20s instead, and on a loaded runner that is
# sometimes not enough: node-2 re-enrols before folding the tombstone, mints the
# SAME device id back (its slot was never released), and the run fails on
# `json_not_equal(replacement_device, lost_device)` — a confusing way to be told
# "you did not wait long enough".
#
# A timeout here is a real failure, not a slow machine to be papered over with a
# bigger number: if the tombstone has not landed in this long, it is not going to.

set -eu

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
    echo "usage: $0 <node> <namespace-id> [timeout-seconds]" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

node="$1"
namespace="$2"
timeout="${3:-120}"

url=$(node_url "${node}")
token=$(node_token "${url}")

echo "waiting up to ${timeout}s for ${node} to release its device in ${namespace}"

elapsed=0
interval=2
while [ "${elapsed}" -lt "${timeout}" ]; do
    # A failed READ is not "no device": treat only a well-formed answer as an
    # answer, and keep polling through a transient blip rather than declaring the
    # revocation landed because curl had a bad second.
    if resp=$(curl -sS --fail-with-body \
            -H "Authorization: Bearer ${token}" \
            "${url}/admin-api/namespaces/${namespace}/account" 2>/dev/null); then
        # `has`, not `jq -e .data.deviceId`. `-e` exits non-zero when the value is
        # `null` — which is precisely the success condition here — so testing the
        # value's truthiness would make a released slot indistinguishable from a
        # malformed response, and the loop would spin to timeout on the happy path.
        # Ask whether the FIELD is there, then read it.
        if printf '%s' "${resp}" | jq -e '.data | has("deviceId")' >/dev/null 2>&1; then
            device=$(printf '%s' "${resp}" | jq -r '.data.deviceId')
            if [ "${device}" = "null" ]; then
                echo "${node} released its device after ${elapsed}s"
                exit 0
            fi
        else
            echo "  (no .data.deviceId in response: ${resp})" >&2
        fi
    fi
    sleep "${interval}"
    elapsed=$((elapsed + interval))
done

echo "${node} still holds a device in ${namespace} after ${timeout}s —" \
     "the revocation tombstone never landed" >&2
printf 'last response: %s\n' "${resp:-<none>}" >&2
exit 1
