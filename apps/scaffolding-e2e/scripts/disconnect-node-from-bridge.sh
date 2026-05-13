#!/bin/sh
#
# Disconnect a merobox-managed node container from the default Docker
# bridge network. Used by partition-simulation workflow steps:
#
#     - name: Partition admin away from peers
#       type: script
#       script: apps/scaffolding-e2e/scripts/disconnect-node-from-bridge.sh
#       target: local
#       args:
#         - e2e-node-1
#
# Effect: the container keeps running and retains all in-memory state,
# but cannot reach (or be reached by) other containers on the default
# bridge. Re-attach with the companion `connect-node-to-bridge.sh`.
#
# Caveat: this is not a perfect partition. Containers also bind to the
# host's exposed ports, so any peer connecting via host gateway would
# still see them. Inside merobox's default 1-host setup this is fine —
# all inter-node libp2p traffic flows over the bridge.

set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <container-name>" >&2
    exit 1
fi

container="$1"
echo "disconnecting ${container} from docker bridge"
docker network disconnect bridge "${container}"
