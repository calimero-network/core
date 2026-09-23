#!/usr/bin/env bash
#
# Tests for check-like-ci.py against a fixture workflow whose steps only echo.
#
#   bash scripts/tests/check-like-ci.test.sh

set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd -P)
SCRIPT="$HERE/../check-like-ci.py"
PASS=0
FAIL=0
# Inside the repo: the script prints the workflow path relative to it.
ROOT=$(mktemp -d "$HERE/check-like-ci.XXXXXX")
trap 'rm -rf "$ROOT"' EXIT

cat >"$ROOT/ci.yml" <<'EOF'
jobs:
  rust:
    needs: [rust-a, rust-b]
    steps:
      - run: echo aggregate-only
  rust-a:
    steps:
      - name: First
        run: echo a
  rust-b:
    steps:
      - uses: taiki-e/install-action@v2
        with:
          tool: definitely-not-installed-tool@1.0.0
      - name: Second
        run: echo b
  other:
    steps:
      - name: Elsewhere
        run: echo other
EOF

# check <label> <want-substring> <args...>
check() {
  local label="$1" want="$2" out
  shift 2
  out=$(python3 "$SCRIPT" --workflow "$ROOT/ci.yml" "$@" 2>&1)
  if printf '%s' "$out" | grep -qF -- "$want"; then
    PASS=$((PASS + 1)); printf '  ok    %s\n' "$label"
  else
    FAIL=$((FAIL + 1)); printf '  FAIL  %s\n     wanted "%s" in:\n%s\n' "$label" "$want" "$out"
  fi
}

# check_absent <label> <unwanted-substring> <args...>
check_absent() {
  local label="$1" unwanted="$2" out
  shift 2
  out=$(python3 "$SCRIPT" --workflow "$ROOT/ci.yml" "$@" 2>&1)
  if printf '%s' "$out" | grep -qF -- "$unwanted"; then
    FAIL=$((FAIL + 1)); printf '  FAIL  %s\n     did not want "%s" in:\n%s\n' "$label" "$unwanted" "$out"
  else
    PASS=$((PASS + 1)); printf '  ok    %s\n' "$label"
  fi
}

check "default runs every job the aggregate needs" "jobs rust-a, rust-b" --list
check "steps are listed in job order" "2. rust-b: Second" --list
check_absent "a job the aggregate does not need is left out" "Elsewhere" --list
check_absent "the aggregate's own step is left out" "aggregate-only" --list
check "--job overrides the default" "1. other: Elsewhere" --list --job other
check "a tool CI installs is reported when missing" "definitely-not-installed-tool" --only Second
check_absent "a tool is not reported for a job with no selected step" \
  "definitely-not-installed-tool" --only First

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
