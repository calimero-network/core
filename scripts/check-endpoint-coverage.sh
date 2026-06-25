#!/usr/bin/env bash
# Fail if any manifest endpoint was NOT exercised by the SDK e2e run — so a new
# endpoint that ships without an SDK test is flagged.
#
# Args:
#   $1  endpoints.json      committed route manifest, e.g. ["/admin-api/contexts/:context_id", ...]
#   $2  covered-endpoints.json   concrete request paths the SDK e2e HttpClient recorded
#   $3  coverage-baseline.json   (optional) accepted-uncovered routes — known gaps
#                                that don't fail the build (the ratchet). A new
#                                uncovered route NOT in the baseline fails.
#
# Recorded concrete paths are matched against manifest patterns (":seg" -> one
# segment, "*rest" -> anything). Query strings on recorded paths are ignored.
set -euo pipefail

MANIFEST="${1:?usage: check-endpoint-coverage.sh <endpoints.json> <covered-endpoints.json> [baseline.json]}"
COVERED="${2:?usage: check-endpoint-coverage.sh <endpoints.json> <covered-endpoints.json> [baseline.json]}"
BASELINE="${3:-}"
command -v jq >/dev/null || { echo "ERROR: jq is required"; exit 1; }

patterns=()
while IFS= read -r line; do patterns+=("$line"); done < <(jq -r '.[]' "$MANIFEST")
hits=()
while IFS= read -r line; do hits+=("$line"); done < <(jq -r '.[]' "$COVERED")
baseline=()
if [ -n "$BASELINE" ] && [ -f "$BASELINE" ]; then
  while IFS= read -r line; do baseline+=("$line"); done < <(jq -r '.[]' "$BASELINE")
fi

is_baselined() {
  local p="$1"
  for b in ${baseline[@]+"${baseline[@]}"}; do [ "$b" = "$p" ] && return 0; done
  return 1
}

new_uncovered=()
baselined_uncovered=()
for pattern in "${patterns[@]}"; do
  # ":seg" -> "[^/]+"; "*rest" -> ".*"
  rx="^$(printf '%s' "$pattern" | sed -E 's#:[^/]+#[^/]+#g; s#\*[^/]+#.*#g')$"
  matched=0
  for hit in "${hits[@]}"; do
    hp="${hit%%\?*}"
    if [[ "$hp" =~ $rx ]]; then matched=1; break; fi
  done
  if [ "$matched" -eq 0 ]; then
    if is_baselined "$pattern"; then baselined_uncovered+=("$pattern"); else new_uncovered+=("$pattern"); fi
  fi
done

if [ "${#baselined_uncovered[@]}" -gt 0 ]; then
  echo "::notice::${#baselined_uncovered[@]} baselined (accepted-uncovered) routes — burndown backlog:"
  printf '  - %s\n' "${baselined_uncovered[@]}"
fi

if [ "${#new_uncovered[@]}" -gt 0 ]; then
  echo "New endpoint(s) with no SDK e2e coverage (add a mero-js e2e test, or add to coverage-baseline.json with a reason):"
  printf '  %s\n' "${new_uncovered[@]}"
  exit 1
fi
covered=$(( ${#patterns[@]} - ${#baselined_uncovered[@]} ))
echo "OK: ${covered}/${#patterns[@]} manifest endpoints exercised; ${#baselined_uncovered[@]} baselined."
