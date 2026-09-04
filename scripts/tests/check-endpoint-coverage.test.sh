#!/usr/bin/env bash
#
# Fixture tests for check-endpoint-coverage.sh. No node, no network.
#
#   bash scripts/tests/check-endpoint-coverage.test.sh
#
# The gate this guards is the only thing standing between a renamed request
# field and a green run, and its failure mode is silent by construction: a
# route the SDK only ever gets a 4xx on looks exactly like a covered one.

set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd -P)
CHECK="$HERE/../check-endpoint-coverage.sh"
[ -f "$CHECK" ] || { echo "cannot find check-endpoint-coverage.sh next to the tests"; exit 1; }

PASS=0
FAIL=0
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

ok()  { PASS=$((PASS + 1)); printf '  ok    %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL  %s\n     %s\n' "$1" "$2"; }

# run <label> <want-exit> <manifest> <covered> <baseline> [substring the output must contain]
run() {
  local label="$1" want="$2" expect="${6:-}" out got
  printf '%s' "$3" > "$ROOT/endpoints.json"
  printf '%s' "$4" > "$ROOT/covered.json"
  printf '%s' "$5" > "$ROOT/baseline.json"
  out=$(bash "$CHECK" "$ROOT/endpoints.json" "$ROOT/covered.json" "$ROOT/baseline.json" 2>&1)
  got=$?
  if [ "$got" -ne "$want" ]; then
    bad "$label" "expected exit $want, got $got: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-200)"
  elif [ -n "$expect" ] && ! printf '%s' "$out" | grep -qF -- "$expect"; then
    bad "$label" "output missing '$expect': $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-200)"
  else
    ok "$label"
  fi
}

ONE='["GET /admin-api/contexts"]'
PARAM='["GET /admin-api/contexts/{context_id}"]'
EMPTY='[]'

echo "legacy string entries (older recorder, status unknown)"
run "a bare string covers its route" 0 \
  "$ONE" '["GET /admin-api/contexts"]' "$EMPTY"
run "a bare string covers a parameterised route" 0 \
  "$PARAM" '["GET /admin-api/contexts/abc123?limit=1"]' "$EMPTY"

echo "status-carrying entries"
run "a 200 covers its route" 0 \
  "$ONE" '[{"route": "GET /admin-api/contexts", "status": 200}]' "$EMPTY"
run "a 4xx-only route is reported as refused, with the statuses seen" 1 \
  "$ONE" '[{"route": "GET /admin-api/contexts", "status": 400},
           {"route": "GET /admin-api/contexts", "status": 404}]' "$EMPTY" \
  "GET /admin-api/contexts (status 400 404)"
run "a 5xx-only route is refused too" 1 \
  "$ONE" '[{"route": "GET /admin-api/contexts", "status": 500}]' "$EMPTY" \
  "every call was refused"
run "one success among failures covers the route" 0 \
  "$ONE" '[{"route": "GET /admin-api/contexts", "status": 400},
           {"route": "GET /admin-api/contexts", "status": 200}]' "$EMPTY"
run "a success on a sibling path does not cover the pattern's method" 1 \
  "$ONE" '[{"route": "POST /admin-api/contexts", "status": 200}]' "$EMPTY" \
  "no SDK e2e coverage"

echo "the baseline ratchet"
run "an untested route is excused by the baseline" 0 \
  "$ONE" "$EMPTY" '["GET /admin-api/contexts"]'
run "an untested route absent from the baseline fails" 1 \
  "$ONE" "$EMPTY" "$EMPTY" "no SDK e2e coverage"
run "the baseline excuses an all-4xx route but still prints its statuses" 0 \
  "$ONE" '[{"route": "GET /admin-api/contexts", "status": 400}]' '["GET /admin-api/contexts"]' \
  "GET /admin-api/contexts (status 400)"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
