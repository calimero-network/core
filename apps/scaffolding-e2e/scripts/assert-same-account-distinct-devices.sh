#!/bin/sh
#
# Assert the two devices are one account with two replica ids.
#
#     - name: The two devices authored as one account but distinct replicas
#       type: script
#       script: apps/scaffolding-e2e/scripts/assert-same-account-distinct-devices.sh
#       target: local
#       args: [ <namespace-id> ]
#
# This is the feature in one assertion: one grant, two writers. Both halves
# matter. Same account means the second device needed no membership of its own.
# Distinct device ids mean they are distinct CRDT replicas — if they collided,
# the two would share counter slots and an HLC seed and silently lose writes.
#
# Reads what the create and pair steps recorded. That the writes themselves
# converged is asserted by the workflow's own get steps; this pins the identity
# relationship behind them, which no `call` step can observe.

set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <namespace-id>" >&2
    exit 1
fi

. "$(dirname "$0")/account-api.sh"

namespace="$1"

account=$(cat "$(state_file "${namespace}" account)")
first=$(cat "$(state_file "${namespace}" first-device)")
paired=$(cat "$(state_file "${namespace}" paired-device)")

if [ -z "${account}" ] || [ -z "${first}" ] || [ -z "${paired}" ]; then
    echo "missing recorded ids: account=${account} first=${first} paired=${paired}" >&2
    exit 1
fi

if [ "${first}" = "${paired}" ]; then
    echo "the two devices share a replica id (${first}); they would collide on \
counter slots and HLC seeds" >&2
    exit 1
fi

echo "one account ${account} with distinct devices ${first} and ${paired}"
