#!/usr/bin/env bash
# Fail if any manifest endpoint was NOT exercised by the SDK e2e run, or was
# exercised but answered >= 400 on every call - a route the SDK only ever gets
# refused on is an untested route wearing a coverage badge.
#
# Args:
#   $1  endpoints.json      committed route manifest, e.g. ["GET /admin-api/contexts/{context_id}", ...]
#   $2  covered-endpoints.json   what the SDK e2e recorded: either {"route": "METHOD /path",
#                                "status": 200} objects, or bare "METHOD /path" strings from an
#                                older recorder (status unknown, taken as a hit)
#   $3  coverage-baseline.json   (optional) accepted-uncovered routes — known gaps
#                                that don't fail the build (the ratchet). A new
#                                uncovered route NOT in the baseline fails.
#
# Entries are method-aware "METHOD /path". A recorded "METHOD /concrete" covers a
# manifest "METHOD /pattern" when the methods match, the path matches the pattern
# ("{seg}" -> one segment, "{*rest}" -> anything), and the response was under 400.
# Query strings are ignored. The baseline excuses a route with no test, never one
# whose every call was refused - that is a broken test, not a missing one.
set -euo pipefail

MANIFEST="${1:?usage: check-endpoint-coverage.sh <endpoints.json> <covered-endpoints.json> [baseline.json]}"
COVERED="${2:?usage: check-endpoint-coverage.sh <endpoints.json> <covered-endpoints.json> [baseline.json]}"
BASELINE="${3:-}"
command -v jq >/dev/null || { echo "ERROR: jq is required"; exit 1; }

patterns=()
while IFS= read -r line; do patterns+=("$line"); done < <(jq -r '.[]' "$MANIFEST")
# Each hit is read as "<status> METHOD /path". A legacy string entry gets -1, which
# compares under 400 and so counts as a successful call, keeping an older recorder working.
hits=()
hit_status=()
while IFS= read -r line; do
  hit_status+=("${line%% *}")
  hits+=("${line#* }")
done < <(jq -r '.[] | if type == "string" then "-1 \(.)" else "\(.status) \(.route)" end' "$COVERED")
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
refused_only=()
for pattern in "${patterns[@]}"; do
  # Split "METHOD /path-pattern".
  pmethod="${pattern%% *}"
  ppath="${pattern#* }"
  # "{seg}" -> "[^/]+"; "{*rest}" -> ".*" (catch-all first, else the generic rule eats it)
  rx="^$(printf '%s' "$ppath" | sed -E 's#\{\*[^{}/]+\}#.*#g; s#\{[^{}/]+\}#[^/]+#g')$"
  matched=0
  refused=""
  for i in "${!hits[@]}"; do
    hit="${hits[$i]}"
    hmethod="${hit%% *}"
    hrest="${hit#* }"
    hp="${hrest%%\?*}"
    hp="${hp%/}"  # trailing slash is path-equivalent (e.g. /contexts/sync/ == /contexts/sync)
    [ "$hmethod" = "$pmethod" ] && [[ "$hp" =~ $rx ]] || continue
    if [ "${hit_status[$i]}" -lt 400 ]; then matched=1; break; fi
    case " $refused " in *" ${hit_status[$i]} "*) ;; *) refused="${refused:+$refused }${hit_status[$i]}" ;; esac
  done
  if [ "$matched" -eq 0 ]; then
    if [ -n "$refused" ]; then refused_only+=("$pattern (status $refused)")
    elif is_baselined "$pattern"; then baselined_uncovered+=("$pattern")
    else new_uncovered+=("$pattern"); fi
  fi
done

if [ "${#baselined_uncovered[@]}" -gt 0 ]; then
  echo "::notice::${#baselined_uncovered[@]} baselined (accepted-uncovered) routes — burndown backlog:"
  printf '  - %s\n' "${baselined_uncovered[@]}"
fi

fail=0
if [ "${#refused_only[@]}" -gt 0 ]; then
  echo "Endpoint(s) exercised but every call was refused (the baseline does not excuse these - fix the SDK call or the route):"
  printf '  %s\n' "${refused_only[@]}"
  fail=1
fi

if [ "${#new_uncovered[@]}" -gt 0 ]; then
  echo "New endpoint(s) with no SDK e2e coverage (add a mero-js e2e test, or add to coverage-baseline.json with a reason):"
  printf '  %s\n' "${new_uncovered[@]}"
  fail=1
fi
[ "$fail" -eq 0 ] || exit 1
covered=$(( ${#patterns[@]} - ${#baselined_uncovered[@]} ))
echo "OK: ${covered}/${#patterns[@]} manifest endpoints exercised; ${#baselined_uncovered[@]} baselined."
