#!/usr/bin/env bash
# Fail if measured storage costs differ from the committed snapshot.
#
# Row counts are deterministic — same inputs, same read/write/remove counts on
# any machine — so ANY delta is a real change and blocks. An improvement blocks
# too, on purpose: the snapshot is the reviewed record of what operations cost,
# and a cost that moves should move visibly.
#
# Byte counts are deliberately NOT in the snapshot. Entity ids are random
# (`Id::random` -> `rand::thread_rng`), so index rows serialize to slightly
# different lengths run to run; gating on them would flake. See the module docs
# of tools/storage-cost/src/lib.rs.
#
# To accept a change: cargo run -p storage-cost --bin storage-cost --release \
#     > tools/storage-cost/storage-costs.json
# and commit it, so the delta appears in the PR diff.
#
# Usage: check-storage-cost.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

SNAPSHOT="tools/storage-cost/storage-costs.json"

if [ ! -f "$SNAPSHOT" ]; then
    echo "ERROR: $SNAPSHOT not found; generate it first (see the header of this script)" >&2
    exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "ERROR: jq is required" >&2
    exit 1
fi

measured="$(mktemp)"
trap 'rm -f "$measured"' EXIT

echo "Measuring storage costs..."
cargo run --quiet -p storage-cost --bin storage-cost --release >"$measured"

# Per-workload tolerance: 0 for almost everything, so almost everything is an
# exact-equality gate. It is nonzero only where walking the whole child trie
# makes the node count follow the random id distribution; tools/storage-cost/
# tests/reproducible.rs re-derives every declared tolerance from live runs, so
# a too-wide band fails there rather than passing here.
#
# A workload present in only one of the two files reports `null` on the missing
# side, which is how an added or deleted workload announces itself instead of
# silently passing.
deltas="$(jq -r -s '
  def num: if . == null then null else . end;
  .[0] as $old | .[1] as $new
  | [ (($old + $new) | keys[]) as $w
      | ((($old[$w].sizes // {}) + ($new[$w].sizes // {})) | keys[]) as $s
      | ["rows_read", "rows_written", "rows_removed"][] as $m
      | { workload: $w, size: $s, metric: $m,
          old: ($old[$w].sizes[$s][$m] | num),
          new: ($new[$w].sizes[$s][$m] | num),
          tol: (($old[$w].tolerance_pct // 0)) }
    ]
  | map(select(
      (.old == null) or (.new == null) or
      # tolerance 0 means exact equality. Anything else would let a one-row
      # drift through on every workload, which is most of them.
      (if .tol == 0
       then .new != .old
       else ((.new - .old) | fabs) > ([1, (.old * .tol / 100)] | max)
       end)
    ))
  | .[]
  | [ .workload, (.size | tonumber | tostring), .metric, (.old | tostring),
      (.new | tostring), (.tol | tostring) ]
  | @tsv
' "$SNAPSHOT" "$measured")"

if [ -z "$deltas" ]; then
    rows="$(jq -r '[.[].sizes | keys[]] | length' "$measured")"
    echo "OK: all $rows measured cost rows match $SNAPSHOT."
    exit 0
fi

echo "" >&2
echo "FAIL: measured storage costs differ from $SNAPSHOT." >&2
echo "" >&2
printf '%-24s %8s %-14s %12s %12s %6s\n' "WORKLOAD" "N" "METRIC" "SNAPSHOT" "MEASURED" "TOL%" >&2
printf '%s\n' "$deltas" | while IFS=$'\t' read -r workload size metric old new tol; do
    printf '%-24s %8s %-14s %12s %12s %6s\n' "$workload" "$size" "$metric" "$old" "$new" "$tol" >&2
done

echo "" >&2
echo "If this change is intended, regenerate and commit the snapshot:" >&2
echo "  cargo run -p storage-cost --bin storage-cost --release > $SNAPSHOT" >&2
echo "so the cost delta lands in the PR diff where a reviewer sees it." >&2
exit 1
