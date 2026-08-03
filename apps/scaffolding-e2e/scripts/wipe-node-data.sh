#!/bin/sh
#
# Delete a node's data directory contents — a lost disk, simulated.
#
#     - name: The disk is gone
#       type: script
#       script: scripts/wipe-node-data.sh
#       target: local
#       args: [ <node-name> ]            # optionally: [ <node-name>, <data-dir> ]
#
# merobox has no wipe step, and this is the one thing a recovery test cannot fake:
# restoring a key into the store it was exported from proves the encoding round
# trips, not that anything was recovered. The account only comes back from the
# phrase if nothing else survived.
#
# Run it AFTER `stop_node`. Note that `stop_node` does not merely stop the node —
# merobox stops *and removes* the container ("Gracefully stopped and removed
# <node>"). So nothing here may depend on the container existing: an earlier
# version asked `docker inspect` both whether it was running and where its mount
# was, and mistook the removed container for a running one.
#
# The data directory therefore comes from merobox's own convention —
# `./data/<node>`, relative to the directory the scenario runs from — or from an
# explicit second argument. Being a host bind mount, it outlives the container,
# which is exactly why the wipe works at all.
#
# Refuses if a container of that name is still running, if the path looks
# dangerous, or if it does not contain the node home it claims to. An `rm -rf`
# assembled from arguments earns every one of those.

set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "usage: $0 <node-name> [data-dir]" >&2
    exit 1
fi

node="$1"
data_dir="${2:-data/${node}}"

# Unambiguous, unlike inspecting a container that may no longer exist: empty
# output means "no running container by that name", whether it was stopped,
# removed, or never created.
running=$(docker ps -q --filter "name=^${node}$" --filter "status=running" 2>/dev/null || true)
if [ -n "${running}" ]; then
    echo "${node} is still running (${running}); stop it before wiping its data" >&2
    exit 1
fi

if [ ! -d "${data_dir}" ]; then
    echo "no data directory at '${data_dir}' (cwd $(pwd)); pass one explicitly" >&2
    exit 1
fi

abs=$(cd "${data_dir}" && pwd)

# Guard the guard: a mistaken argument must not become an rm -rf near the root.
case "${abs}" in
    /|/*/) echo "refusing to wipe '${abs}'" >&2; exit 1 ;;
esac
if [ "$(printf '%s' "${abs}" | wc -c)" -lt 10 ]; then
    echo "refusing to wipe suspiciously short path '${abs}'" >&2
    exit 1
fi
# `<data_dir>/<node>` is merod's home. Its absence means this is not the
# directory we think it is, and wiping it would destroy something else quietly.
if [ ! -d "${abs}/${node}" ]; then
    echo "'${abs}' does not contain ${node}'s home; refusing to wipe it" >&2
    exit 1
fi

echo "wiping ${abs} (contents only; the bind-mount point stays)"
find "${abs}" -mindepth 1 -maxdepth 1 -exec rm -rf {} +

remaining=$(find "${abs}" -mindepth 1 | wc -l | tr -d ' ')
if [ "${remaining}" != "0" ]; then
    echo "wipe left ${remaining} entries behind in ${abs}" >&2
    exit 1
fi

echo "${node}'s data directory is empty — the disk is gone"
