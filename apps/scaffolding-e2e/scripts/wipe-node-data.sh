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
# What is left behind is an EMPTY node home directory — the same state merobox
# leaves before it runs `merod init`. Nothing informative survives (no config, no
# identity, no datastore, no blobs), so "only the phrase survived" still holds; the
# directory exists so that `merod init` and `node_exec` find what they expect.
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

# The node container runs as ROOT (merobox overrides the image's user), so some of
# what it wrote into this bind mount — `blobs/*`, `auth.kek` — is root-owned and
# the user running this script cannot remove it:
#
#     rm: cannot remove '.../blobs/7Stwoeo6…': Permission denied
#
# So: try as ourselves, and escalate only if that fails and passwordless sudo is
# available. Plain removal is enough where the Docker file-sharing layer maps
# ownership back to the local user (macOS); CI is the case that needs sudo.
echo "wiping ${abs} (contents only; the bind-mount point stays)"
if ! find "${abs}" -mindepth 1 -maxdepth 1 -exec rm -rf {} + 2>/dev/null; then
    if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
        echo "some entries are root-owned (written by the node container); using sudo"
        sudo find "${abs}" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
    else
        echo "cannot remove root-owned entries under ${abs} and passwordless sudo \
is unavailable; the node container runs as root and its blob files are owned by it" >&2
        exit 1
    fi
fi

remaining=$(find "${abs}" -mindepth 1 | wc -l | tr -d ' ')
if [ "${remaining}" != "0" ]; then
    echo "wipe left ${remaining} entries behind in ${abs}" >&2
    exit 1
fi

# Recreate the node's home as an EMPTY directory, the state merobox itself leaves
# before running `merod init` (`run_node` makedirs it, then inits into it). An
# empty directory carries nothing — no config, no identity, no datastore, no blobs
# — so the recovery claim ("only the phrase survived") still holds, while
# `merod init` and `node_exec` both have the directory they expect to find.
mkdir -p "${abs}/${node}"
chmod 700 "${abs}" "${abs}/${node}"

echo "${node}'s data is gone; an empty home remains at ${abs}/${node}"
