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
# also does a bounded search of the workspace (deduplicating against the primary
# pass) and shouts loudly if it finds nothing.
#
# Usage: harvest-host-profiling.sh <src-root> <dest-root>
#   <src-root>  primary location to look under, e.g. data
#   <dest-root> where to copy the dumps, e.g. profiling-data/kv-store
#
# Best-effort: never exits non-zero (profiling is diagnostic, not gating).

set -u

SRC_ROOT="${1:?Error: src-root is required}"
DEST_ROOT="${2:?Error: dest-root is required}"

# Bound the fallback search to the checkout. On GHA $GITHUB_WORKSPACE is the
# repo root; outside CI fall back to $PWD. Searching only here keeps us from
# scooping up a stray `profiling-dump` dir from elsewhere on a reused runner.
# Guard against a bogus value (empty, `/`, or a non-dir) so the sweep can't
# end up traversing the whole filesystem.
WORKSPACE="${GITHUB_WORKSPACE:-$PWD}"
case "$WORKSPACE" in ""|"/") WORKSPACE="$PWD" ;; esac
[ -d "$WORKSPACE" ] || WORKSPACE="$PWD"

ERR_LOG=$(mktemp -t harvest-host-profiling.XXXXXX.err)
trap 'rm -f "$ERR_LOG"' EXIT

found=0
HARVESTED=""   # newline-delimited absolute paths of dumps already taken — dedup key

# Print $1 with the $WORKSPACE prefix stripped, so logs stay tidy.
rel() { local p="$1"; printf '%s' "${p#"$WORKSPACE"/}"; }

# Copy one `<dump>` (a `.../profiling-dump` dir) into $DEST_ROOT/<node-name>/.
harvest_dump() {
    local dump="$1" node_name="$2"
    local dest="$DEST_ROOT/$node_name"
    local key err size perf_count heap_count
    # Canonical path so the same dump reached two ways (primary vs sweep)
    # dedups; falls back to the literal path only if the dir vanished, in
    # which case the copy below fails harmlessly anyway.
    key=$(cd "$dump" 2>/dev/null && pwd -P) || key="$dump"
    # `grep -qxF` (whole-line, fixed-string) — robust if the path ever
    # contains shell-pattern metacharacters, unlike a `case` glob match.
    # Pipe instead of a `<<<` here-string so this works under a POSIX `sh`
    # too, not just bash. (`find`/`while read` already can't survive a path
    # with an embedded newline, so the dedup string can't either — that's a
    # pre-existing constraint on the whole workspace, not introduced here.)
    printf '%s\n' "$HARVESTED" | grep -qxF -- "$key" && return   # already taken
    HARVESTED="${HARVESTED}${key}
"
    if ! mkdir -p "$dest" 2>"$ERR_LOG"; then
        err=$(head -3 "$ERR_LOG" 2>/dev/null | tr '\n' ' ')
        echo "  $node_name: ERROR — could not create $dest: ${err:-(no stderr captured)}"
        return
    fi
    if ! cp -r "$dump/." "$dest/" 2>"$ERR_LOG"; then
        err=$(head -3 "$ERR_LOG" 2>/dev/null | tr '\n' ' ')
        echo "  $node_name: WARNING — cp may be incomplete: ${err:-(no stderr captured)}"
    fi
    # Count this as harvested only if something actually landed — a copy that
    # failed outright leaves $dest empty and shouldn't suppress the
    # zero-harvest warning below.
    if [ -z "$(ls -A "$dest" 2>/dev/null)" ]; then
        echo "  $node_name: WARNING — nothing copied from $(rel "$dump"); not counting as harvested"
        return
    fi
    size=$(du -sh "$dest" 2>/dev/null | awk '{print $1}')
    perf_count=$(find "$dest" -maxdepth 2 -name 'perf-*.data' 2>/dev/null | wc -l | tr -d ' ')
    heap_count=$(find "$dest" -maxdepth 2 -name 'jemalloc.*.heap' 2>/dev/null | wc -l | tr -d ' ')
    echo "  $node_name: ${size:-?} (perf.data=$perf_count, heap=$heap_count)  <- $(rel "$dump")"
    found=$((found + 1))
}

# 1) Primary location: <src-root>/<node>/profiling-dump
if [ -d "$SRC_ROOT" ]; then
    echo "Harvesting from primary src-root '$SRC_ROOT':"
    for node_dir in "$SRC_ROOT"/*/; do
        [ -d "$node_dir" ] || continue
        node_name="$(basename "${node_dir%/}")"
        dump="${node_dir%/}/profiling-dump"
        if [ -d "$dump" ]; then
            harvest_dump "$dump" "$node_name"
        else
            # Per-node diagnostic so partial harvests (some nodes profiled, some
            # not) are visible in the log instead of being silently dropped.
            echo "  $node_name: no profiling-dump dir — skipping"
        fi
    done
else
    echo "Primary src-root '$SRC_ROOT' does not exist — relying on the workspace search."
fi

# 2) Always sweep for any other `*/profiling-dump` dirs (the data dir is
#    relative to merobox's CWD and has drifted before). `harvest_dump` dedups
#    against the primary pass by canonical path, so re-finding the same dirs is
#    harmless; this also catches the case where the primary pass found *some*
#    nodes but others landed elsewhere. The sweep is bounded two ways:
#      - `find` only descends `$WORKSPACE/data` and `$WORKSPACE/workflows` —
#        the only two places merobox could plausibly write node data dirs
#        (`./data/<node>` from the repo root, or — in the pre-#2278 layout —
#        `workflows/fuzzy-tests/<suite>/data/<node>`; hence maxdepth 5:
#        fuzzy-tests / <suite> / data / <node> / profiling-dump). Scoping the
#        `find` itself (rather than scanning all of `$WORKSPACE` and filtering
#        after) keeps us out of `crates/`, `target/`, etc. and means a stray
#        `profiling-dump` from elsewhere on a reused runner can't bleed in.
#        `find`'s default `-P` doesn't follow a symlinked start dir, so a
#        symlinked `data/`/`workflows/` would just yield nothing, not escape.
#      - the dump's parent dir must be named `fuzzy-*` (merobox names every
#        fuzzy node `fuzzy-<suite>-node-N`).
while IFS= read -r dump; do
    [ -d "$dump" ] || continue
    node_name=$(basename "$(dirname "$dump")")
    case "$node_name" in fuzzy-*) ;; *) continue ;; esac
    harvest_dump "$dump" "$node_name"
done < <(
    for sweep_root in "$WORKSPACE/data" "$WORKSPACE/workflows"; do
        [ -d "$sweep_root" ] && find "$sweep_root" -maxdepth 5 -type d -name profiling-dump 2>/dev/null
    done
)

if [ "$found" -eq 0 ]; then
    echo "::warning::harvest-host-profiling: harvested 0 profiling-dump dirs (src-root='$SRC_ROOT', workspace='$WORKSPACE')."
    echo "  Profiling-related files present under the workspace:"
    # Surface what *is* there (paths relative to the workspace) so a future
    # path drift is obvious from the run log. Scope the diagnostic search to
    # the same two roots the main sweep uses (`$WORKSPACE/data` +
    # `$WORKSPACE/workflows`) so we don't traverse `crates/`, `target/`, etc.
    # — same bounding rationale as the main sweep, just for diagnostics.
    for diag_root in "$WORKSPACE/data" "$WORKSPACE/workflows"; do
        [ -d "$diag_root" ] || continue
        find "$diag_root" -maxdepth 5 \( -name 'perf-*.data' -o -name 'profiling-dump' -o -name 'jemalloc.*.heap' \) 2>/dev/null
    done | head -40 | while IFS= read -r p; do echo "    $(rel "$p")"; done
    if [ -d "$WORKSPACE/data" ]; then
        echo "  ./data tree (maxdepth 3):"
        find "$WORKSPACE/data" -maxdepth 3 2>/dev/null | head -40 | while IFS= read -r p; do echo "    $(rel "$p")"; done
    fi
fi

echo "Harvested profiling dumps from $found node(s)."
exit 0
