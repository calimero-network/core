#!/usr/bin/env bash
#
# Tests for touch-changed-since.sh on a throwaway repository.
#
#   bash scripts/tests/touch-changed-since.test.sh

set -uo pipefail

SCRIPT="$(cd "$(dirname "$0")/.." && pwd -P)/touch-changed-since.sh"
PASS=0
FAIL=0
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

check() { # name, condition exit status
  if [ "$2" -eq 0 ]; then PASS=$((PASS + 1)); else FAIL=$((FAIL + 1)); echo "FAIL: $1"; fi
}
old() { git ls-files -z | xargs -0 touch -c -h -d '2000-01-01T00:00:00Z'; }
untouched() { [ "$(stat -c %Y "$1" 2>/dev/null || stat -f %m "$1")" -lt 1000000000 ]; }

git init -q "$ROOT/repo" && cd "$ROOT/repo" || exit 1
git config user.name t && git config user.email t@t
echo 1 >same && echo 1 >edited && echo 1 >removed
git add -A && git commit -qm first
first=$(git rev-parse HEAD)
echo 2 >edited && echo 2 >added && git rm -q removed
git add -A && git commit -qm second
second=$(git rev-parse HEAD)

marker="$ROOT/marker/commit"
mkdir -p "$ROOT/marker" && echo "$first" >"$marker"
old
bash "$SCRIPT" "$marker" >/dev/null
! untouched edited; check "a file edited since the cached commit is touched" $?
! untouched added; check "a file added since it is touched" $?
untouched same; check "a file unchanged since it keeps its old mtime" $?
[ "$(cat "$marker")" = "$second" ]; check "the marker records HEAD" $?

old
rm "$marker"
bash "$SCRIPT" "$marker" >/dev/null
! untouched same; check "with no marker every file is touched" $?
[ "$(cat "$marker")" = "$second" ]; check "a missing marker is created at HEAD" $?

echo 0000000000000000000000000000000000000000 >"$marker"
old
bash "$SCRIPT" "$marker" >/dev/null 2>&1
! untouched same; check "with an unreachable commit every file is touched" $?

echo "touch-changed-since: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
