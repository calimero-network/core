#!/bin/bash
# Harvest profiling dumps from host-side bind mounts.
#
# Runtime containers (fuzzy-*-node-N) are removed during merobox's
# graceful shutdown before any Docker-based collector can reach them,
# so /profiling/data inside the container vanishes with it.
# entrypoint-profiling.sh mirrors that data into /app/data/profiling-dump
# on shutdown — and merobox bind-mounts /app/data to the host (see
# merobox manager.py: `{host_path: {bind: /app/data}}`). This script
# harvests those dumps from the host filesystem, regardless of whether
# the runtime container still exists.
#
# Usage: harvest-host-profiling.sh <src-root> <dest-root>
#   <src-root>  e.g. workflows/fuzzy-tests/kv-store/data
#   <dest-root> e.g. profiling-data/kv-store

set -u

SRC_ROOT="${1:?Error: src-root is required}"
DEST_ROOT="${2:?Error: dest-root is required}"

if [ ! -d "$SRC_ROOT" ]; then
    echo "No data dir at $SRC_ROOT — nothing to harvest"
    exit 0
fi

ERR_LOG=$(mktemp -t harvest-host-profiling.XXXXXX.err)
trap 'rm -f "$ERR_LOG"' EXIT

found=0
for node_dir in "$SRC_ROOT"/*/; do
    [ -d "$node_dir" ] || continue
    node_name=$(basename "$node_dir")
    dump="$node_dir/profiling-dump"
    if [ ! -d "$dump" ]; then
        echo "  $node_name: no profiling-dump (runtime container may not have used profiling entrypoint)"
        continue
    fi
    dest="$DEST_ROOT/$node_name"
    if ! mkdir -p "$dest" 2>"$ERR_LOG"; then
        err=$(head -3 "$ERR_LOG" 2>/dev/null | tr '\n' ' ')
        echo "  $node_name: ERROR — could not create $dest: ${err:-(no stderr captured)}"
        continue
    fi
    if ! cp -r "$dump/." "$dest/" 2>"$ERR_LOG"; then
        err=$(head -3 "$ERR_LOG" 2>/dev/null | tr '\n' ' ')
        echo "  $node_name: WARNING — cp may be incomplete: ${err:-(no stderr captured)}"
    fi
    size=$(du -sh "$dest" 2>/dev/null | awk '{print $1}')
    perf_count=$(find "$dest" -maxdepth 2 -name 'perf-*.data' 2>/dev/null | wc -l | tr -d ' ')
    heap_count=$(find "$dest" -maxdepth 2 -name 'jemalloc.*.heap' 2>/dev/null | wc -l | tr -d ' ')
    echo "  $node_name: ${size:-?} (perf.data=$perf_count, heap=$heap_count)"
    found=$((found + 1))
done
echo "Harvested profiling dumps from $found nodes"
