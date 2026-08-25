#!/bin/sh
#
# The application a paired device ends up holding, and the namespace it speaks in
# - both read from the device's OWN admin API.
#
#     - name: The paired device acquired app A's bytecode
#       type: script
#       script: scripts/account-device-application.sh
#       target: local
#       args: [ <device-node>, <application-id>, <namespace-id> ]
#
# Two claims, and the first is the one that could not be made before this branch.
# A device of a member's account follows no context, so it never reaches the
# context bootstrap that installs an application - it holds the row the
# `ContextRegistered` apply wrote and nothing to run. Acquisition drives the
# joiner's chain off that row instead, and the blob reaches a node that is a
# member of nothing because a context's own application bundle is served to any
# requester.
#
# Polled rather than barriered. `wait_for_sync` settles a DAG between nodes; this
# waits on a best-effort acquisition that runs off an apply-time event and
# reports "not yet" by design, so the only honest gate is to ask the device until
# it has it.

set -eu

# Long enough for the registration to apply, the blob to be requested from a peer
# and the bundle to be installed; short enough that a genuine failure is not a
# five-minute wait.
ATTEMPTS=45
INTERVAL=2

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <device-node> <application-id> <namespace-id>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

devicenode="$1"
application="$2"
namespace="$3"

attempt=0
while :; do
    attempt=$((attempt + 1))
    installed=$(api_get "${devicenode}" "applications")
    # `size > 0`, not merely the id being listed. The `ContextRegistered` apply
    # writes a STUB row under the real application id on every replica that can
    # decrypt it - the creator's blob and nothing else - so the id alone is
    # present from the moment the registration lands and would pass this the
    # instant it did. A zero size is the same still-a-stub sentinel the
    # acquisition itself keys on.
    if echo "${installed}" | jq -e --arg a "${application}" \
        'any(.data.apps[]; .id == $a and .size > 0)' >/dev/null; then
        echo "${devicenode} holds ${application} after ${attempt} attempt(s)"
        break
    fi
    if [ "${attempt}" -ge "${ATTEMPTS}" ]; then
        echo "${devicenode} never acquired ${application}; it holds: $(echo "${installed}" | jq -c '[.data.apps[] | {id, size}]')" >&2
        exit 1
    fi
    sleep "${INTERVAL}"
done

# The account-level view of the same fact, and it answers for a node that is a
# member of nothing - which `GET /admin-api/namespaces` deliberately does not,
# because a namespace summary is withheld from non-members. This is the only
# route by which a paired device can be told which applications its account
# speaks in.
applications=$(api_get "${devicenode}" "account/applications")
entry=$(echo "${applications}" | jq -c --arg a "${application}" \
    '.applications[] | select(.applicationId == $a)')
if [ -z "${entry}" ]; then
    echo "${devicenode} does not report ${application} as an application of its account: ${applications}" >&2
    exit 1
fi
if ! echo "${entry}" | jq -e --arg n "${namespace}" 'any(.namespaces[]; . == $n)' >/dev/null; then
    echo "${devicenode} reports ${application} without ${namespace}: ${entry}" >&2
    exit 1
fi

echo "${devicenode} speaks in ${application} through ${namespace}, holding neither membership nor a context"
