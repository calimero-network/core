#!/usr/bin/env bash
# Usage: check-storage-cost.sh
#
# Fails if measured storage costs differ from the committed snapshot. Row
# counts are deterministic, so any delta blocks - an improvement too, since the
# snapshot is the reviewed record of what an operation costs. To accept one:
#
#     cargo run -p storage-cost --bin storage-cost --release \
#         > tools/storage-cost/storage-costs.json
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

# Tolerance is 0 for almost every workload, so almost every row is an exact
# equality. A workload present in only one file reports `null` on the other
# side, which is how an added or deleted workload announces itself.
deltas="$(jq -r -s '
  .[0] as $old | .[1] as $new
  | [ (($old + $new) | keys[]) as $w
      | ((($old[$w].sizes // {}) + ($new[$w].sizes // {})) | keys[]) as $s
      | ["rows_read", "rows_written", "rows_removed"][] as $m
      | { workload: $w, size: $s, metric: $m,
          old: $old[$w].sizes[$s][$m],
          new: $new[$w].sizes[$s][$m],
          tol: (($old[$w].tolerance_pct // 0)) }
    ]
  | map(select(
      (.old == null) or (.new == null) or
      # tolerance 0 means exact equality
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
