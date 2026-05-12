#!/bin/sh
#
# Re-attach a merobox-managed node container to the default Docker
# bridge network. Companion to `disconnect-node-from-bridge.sh`.
#
#     - name: Heal admin's partition
#       type: script
#       script: apps/scaffolding-e2e/scripts/connect-node-to-bridge.sh
#       target: local
#       args:
#         - e2e-node-1
#
# After reconnect, gossipsub mesh reformation typically takes 5–10s
# (libp2p heartbeats + peer discovery), so workflows should follow
# this step with an explicit `wait_for_sync` or short `wait` before
# asserting state propagation.

set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <container-name>" >&2
    exit 1
fi

container="$1"
echo "reconnecting ${container} to docker bridge"
docker network connect bridge "${container}"
