#!/bin/sh
#
# Delete a stopped node's data directory contents — a lost disk, simulated.
#
#     - name: The disk is gone
#       type: script
#       script: scripts/wipe-node-data.sh
#       target: local
#       args: [ <container> ]
#
# merobox has no wipe step, and this is the one thing a recovery test cannot fake:
# restoring a key into the store it was exported from proves the encoding round
# trips, not that anything was recovered. The account only comes back from the
# phrase if nothing else survived.
#
# The node must be STOPPED — deleting a live RocksDB out from under merod gets a
# crash rather than a clean start. The data directory is a host bind mount, so
# this deletes the CONTENTS on the host side; removing the directory itself would
# break the mount and the container would come back with nothing to write into.
#
# Refuses on a running container, an unresolvable mount, or a suspiciously short
# path — an `rm -rf` driven by `docker inspect` output deserves all three.

set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <container>" >&2
    exit 1
fi

container="$1"

running=$(docker inspect -f '{{.State.Running}}' "${container}" 2>/dev/null || echo "missing")
if [ "${running}" = "missing" ]; then
    echo "no such container: ${container}" >&2
    exit 1
fi
if [ "${running}" != "false" ]; then
    echo "${container} is still running; stop it before wiping its data" >&2
    exit 1
fi

source_dir=$(docker inspect \
    -f '{{range .Mounts}}{{if eq .Destination "/app/data"}}{{.Source}}{{end}}{{end}}' \
    "${container}")

if [ -z "${source_dir}" ]; then
    echo "${container} has no bind mount at /app/data" >&2
    exit 1
fi

# Guard the guard: a truncated or unexpected inspect result must not become an
# `rm -rf` near the filesystem root.
case "${source_dir}" in
    /|/*/) echo "refusing to wipe '${source_dir}'" >&2; exit 1 ;;
esac
if [ "$(printf '%s' "${source_dir}" | wc -c)" -lt 10 ]; then
    echo "refusing to wipe suspiciously short path '${source_dir}'" >&2
    exit 1
fi
if [ ! -d "${source_dir}" ]; then
    echo "'${source_dir}' is not a directory" >&2
    exit 1
fi

echo "wiping ${source_dir} (the contents; the mount point stays)"
# Contents including dotfiles, without deleting the mount point itself.
find "${source_dir}" -mindepth 1 -maxdepth 1 -exec rm -rf {} +

remaining=$(find "${source_dir}" -mindepth 1 | wc -l | tr -d ' ')
if [ "${remaining}" != "0" ]; then
    echo "wipe left ${remaining} entries behind in ${source_dir}" >&2
    exit 1
fi

echo "${container}'s data directory is empty — the disk is gone"
