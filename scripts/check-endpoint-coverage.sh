#!/usr/bin/env bash
# Fail if any manifest endpoint was NOT exercised by the SDK e2e run — so a new
# endpoint that ships without an SDK test is flagged.
#
# Args:
#   $1  endpoints.json      committed route manifest, e.g. ["/admin-api/contexts/:context_id", ...]
#   $2  covered-endpoints.json   concrete request paths the SDK e2e HttpClient recorded
#
# Recorded concrete paths are matched against manifest patterns (":seg" -> one
# segment, "*rest" -> anything). Query strings on recorded paths are ignored.
set -euo pipefail

MANIFEST="${1:?usage: check-endpoint-coverage.sh <endpoints.json> <covered-endpoints.json>}"
COVERED="${2:?usage: check-endpoint-coverage.sh <endpoints.json> <covered-endpoints.json>}"
command -v jq >/dev/null || { echo "ERROR: jq is required"; exit 1; }

patterns=()
while IFS= read -r line; do patterns+=("$line"); done < <(jq -r '.[]' "$MANIFEST")
hits=()
while IFS= read -r line; do hits+=("$line"); done < <(jq -r '.[]' "$COVERED")

uncovered=()
for pattern in "${patterns[@]}"; do
  # ":seg" -> "[^/]+"; "*rest" -> ".*"
  rx="^$(printf '%s' "$pattern" | sed -E 's#:[^/]+#[^/]+#g; s#\*[^/]+#.*#g')$"
  matched=0
  for hit in "${hits[@]}"; do
    hp="${hit%%\?*}"
    if [[ "$hp" =~ $rx ]]; then matched=1; break; fi
  done
  [ "$matched" -eq 0 ] && uncovered+=("$pattern")
done

if [ "${#uncovered[@]}" -gt 0 ]; then
  echo "Endpoints with no SDK e2e coverage (add a mero-js e2e test or remove the route):"
  printf '  %s\n' "${uncovered[@]}"
  exit 1
fi
echo "All ${#patterns[@]} manifest endpoints were exercised by the SDK e2e run."
