#!/bin/bash
# Lift the rendered profiling reports (flamegraph SVGs etc.) out of the
# harvested per-node data into a small standalone artifact dir.
#
# entrypoint-profiling.sh's preserve_to_host_mount renders
# flamegraph-{cpu,memory}-<node>.svg into the node's profiling-dump/reports/
# dir on the bind mount; harvest-host-profiling.sh then copies that whole
# profiling-dump/ — reports/ subdir included — into <src>/<node>/. This pulls
# just those reports/ trees into <dst>/<node>/, so a `profiling-reports-*`
# artifact carries only the rendered profile while the heavy raw `perf-*.data`
# / `*.heap` data stays in `profiling-data-*`.
#
# (This used to be done by `collect-from-containers.sh`'s `docker cp
# /profiling/reports` — but `merobox bootstrap run` tears the runtime
# containers down before that step runs, so it always copied nothing; hence
# the perpetually-empty `profiling-reports-*` artifact. Lifting from the
# already-harvested host data instead is the fix.)
#
# Usage: lift-reports.sh <harvested-data-root> <reports-dest>
#   e.g. lift-reports.sh profiling-data/kv-store profiling-reports/kv-store
#
# Best-effort: never exits non-zero (profiling is diagnostic, not gating).

set -u

SRC="${1:?Error: harvested-data-root is required}"
DST="${2:?Error: reports-dest is required}"

mkdir -p "$DST"
found=0

if [ -d "$SRC" ]; then
    # -maxdepth 2 restricts to `<src>/<node>/reports` only — anything deeper is
    # not a per-node reports tree and could be a stray dir from prior content,
    # so don't pull it into the published artifact.
    while IFS= read -r reports_dir; do
        [ -d "$reports_dir" ] || continue
        [ -n "$(ls -A "$reports_dir" 2>/dev/null)" ] || continue
        node=$(basename "$(dirname "$reports_dir")")
        mkdir -p "$DST/$node"
        cp -r "$reports_dir/." "$DST/$node/" 2>/dev/null || true
        # Only count as found if `cp` actually produced content; an empty
        # destination dir means cp failed (e.g. permission denied) — the
        # `|| true` would otherwise silently inflate `$found`.
        if [ -n "$(ls -A "$DST/$node" 2>/dev/null)" ]; then
            found=$((found + 1))
        else
            rmdir "$DST/$node" 2>/dev/null || true
        fi
    done < <(find "$SRC" -maxdepth 2 -type d -name reports 2>/dev/null)
fi

if [ "$found" -gt 0 ]; then
    echo "Lifted rendered profiling reports from $found node(s) -> $DST:"
    find "$DST" -type f 2>/dev/null | sed 's/^/  /'
else
    echo "::notice::lift-reports: no rendered profiling reports under $SRC/*/reports/ — the in-container flamegraph render produced nothing this run; raw perf/heap data is still in the profiling-data-* artifact."
fi

exit 0
