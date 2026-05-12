#!/bin/bash
# Harvest profiling dumps from host-side bind mounts.
#
# Runtime containers (fuzzy-*-node-N) are removed during merobox's
# graceful shutdown before any Docker-based collector can reach them,
# so /profiling/data inside the container vanishes with it.
# entrypoint-profiling.sh mirrors that data to $CALIMERO_HOME/profiling-dump
# on shutdown — and merobox passes CALIMERO_HOME=/app/data plus a bind
# mount of the per-node host data dir (manager.py: `data_dir = "./data/<node>"`,
# then `{abspath(data_dir): {bind: "/app/data"}}`). That `./data` is relative
# to merobox's *working directory* — merobox does not chdir to the workflow
# file's directory — so when `fuzzy-load-test.yml` runs `merobox bootstrap run`
# from the repo root the dumps land under `<repo-root>/data/<node>/profiling-dump/`
# (it used to be `workflows/fuzzy-tests/<test>/data/<node>/...` back when the
# job did `cd workflows/fuzzy-tests/<test>` first — see #2278). This script
# harvests those dumps regardless of whether the runtime container still exists,
# and — because the exact location has drifted before and been hard to notice —
# falls back to a bounded search and shouts loudly if it finds nothing.
#
# Usage: harvest-host-profiling.sh <src-root> <dest-root>
#   <src-root>  primary location to look under, e.g. data
#   <dest-root> where to copy the dumps, e.g. profiling-data/kv-store
#
# Best-effort: never exits non-zero (profiling is diagnostic, not gating).

set -u

SRC_ROOT="${1:?Error: src-root is required}"
DEST_ROOT="${2:?Error: dest-root is required}"

ERR_LOG=$(mktemp -t harvest-host-profiling.XXXXXX.err)
trap 'rm -f "$ERR_LOG"' EXIT

found=0

# Copy one `<node-dir>/profiling-dump` tree into $DEST_ROOT/<node-name>/.
harvest_dump() {
    local dump="$1" node_name="$2"
    local dest="$DEST_ROOT/$node_name"
    if ! mkdir -p "$dest" 2>"$ERR_LOG"; then
        local err
        err=$(head -3 "$ERR_LOG" 2>/dev/null | tr '\n' ' ')
        echo "  $node_name: ERROR — could not create $dest: ${err:-(no stderr captured)}"
        return
    fi
    if ! cp -r "$dump/." "$dest/" 2>"$ERR_LOG"; then
        local err
        err=$(head -3 "$ERR_LOG" 2>/dev/null | tr '\n' ' ')
        echo "  $node_name: WARNING — cp may be incomplete: ${err:-(no stderr captured)}"
    fi
    local size perf_count heap_count
    size=$(du -sh "$dest" 2>/dev/null | awk '{print $1}')
    perf_count=$(find "$dest" -maxdepth 2 -name 'perf-*.data' 2>/dev/null | wc -l | tr -d ' ')
    heap_count=$(find "$dest" -maxdepth 2 -name 'jemalloc.*.heap' 2>/dev/null | wc -l | tr -d ' ')
    echo "  $node_name: ${size:-?} (perf.data=$perf_count, heap=$heap_count)  <- $dump"
    found=$((found + 1))
}

# 1) Primary location: <src-root>/<node>/profiling-dump
if [ -d "$SRC_ROOT" ]; then
    for node_dir in "$SRC_ROOT"/*/; do
        [ -d "$node_dir" ] || continue
        dump="${node_dir%/}/profiling-dump"
        [ -d "$dump" ] || continue
        harvest_dump "$dump" "$(basename "${node_dir%/}")"
    done
else
    echo "Primary src-root '$SRC_ROOT' does not exist — will fall back to a search."
fi

# 2) Fallback: bounded search for any */profiling-dump we haven't taken yet.
#    Covers merobox's data dir landing somewhere other than <src-root>
#    (it's relative to merobox's CWD, which has changed before).
if [ "$found" -eq 0 ]; then
    echo "Nothing under '$SRC_ROOT' — searching for profiling-dump dirs under '$PWD' (maxdepth 5)..."
    while IFS= read -r dump; do
        [ -d "$dump" ] || continue
        parent=$(dirname "$dump")
        harvest_dump "$dump" "$(basename "$parent")"
    done < <(find "$PWD" -maxdepth 5 -type d -name profiling-dump 2>/dev/null)
fi

if [ "$found" -eq 0 ]; then
    echo "::warning::harvest-host-profiling: harvested 0 profiling-dump dirs (src-root='$SRC_ROOT', cwd='$PWD')."
    echo "  Candidates that exist on the runner:"
    # Surface what *is* there so a future path drift is obvious from the log.
    find "$PWD" -maxdepth 5 \( -name 'perf-*.data' -o -name 'profiling-dump' -o -name 'jemalloc.*.heap' \) 2>/dev/null | head -40 | sed 's/^/    /'
    [ -d "$PWD/data" ] && { echo "  Contents of ./data:"; ls -laR "$PWD/data" 2>/dev/null | head -40 | sed 's/^/    /'; }
fi

echo "Harvested profiling dumps from $found node(s)."
exit 0
