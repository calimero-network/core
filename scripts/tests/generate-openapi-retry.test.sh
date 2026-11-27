#!/usr/bin/env bash
#
# Tests that mero-swift's generate-openapi.sh retries the generator's two fetches
# (git clone, swift package resolve), with `git` and `swift` stubbed on PATH.
#
#   bash scripts/tests/generate-openapi-retry.test.sh

set -uo pipefail

SCRIPT="$(cd "$(dirname "$0")/../.." && pwd -P)/sdks/clients/mero-swift/scripts/generate-openapi.sh"
SPEC="$(cd "$(dirname "$0")/../.." && pwd -P)/docs/public/openapi.json"
PASS=0
FAIL=0
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

check() { # name, condition exit status
  if [ "$2" -eq 0 ]; then PASS=$((PASS + 1)); else FAIL=$((FAIL + 1)); echo "FAIL: $1"; fi
}

mkdir "$ROOT/bin"
cat >"$ROOT/bin/git" <<'STUB'
#!/usr/bin/env bash
# `git clone` fails the first $CLONE_FAILURES times, then makes the checkout.
echo clone >>"$CALLS"
[ "$(grep -c '^clone$' "$CALLS")" -gt "$CLONE_FAILURES" ] || exit 128
for last; do :; done
mkdir -p "$last"
STUB
cat >"$ROOT/bin/swift" <<'STUB'
#!/usr/bin/env bash
echo "swift $1 $2" >>"$CALLS"
case "$1 $2" in
  "package resolve")
    [ "$(grep -c '^swift package resolve$' "$CALLS")" -gt "$RESOLVE_FAILURES" ] || exit 1 ;;
  "build -c")
    mkdir -p .build/release
    printf '#!/bin/sh\necho generator >>"$CALLS"\nexit 1\n' >.build/release/swift-openapi-generator
    chmod +x .build/release/swift-openapi-generator ;;
esac
STUB
chmod +x "$ROOT/bin/git" "$ROOT/bin/swift"

# The stub generator fails, so a run that reaches it ends there: what matters is
# which fetches ran, and how often.
run() { # clone failures, resolve failures
  : >"$ROOT/calls"
  CALLS="$ROOT/calls" CLONE_FAILURES=$1 RESOLVE_FAILURES=$2 RETRY_DELAY=0 \
    MERO_OPENAPI_SPEC="$SPEC" MERO_OPENAPI_CACHE="$ROOT/cache.$1.$2" PATH="$ROOT/bin:$PATH" \
    bash "$SCRIPT" generate >"$ROOT/out" 2>&1
}
count() { grep -c "^$1\$" "$ROOT/calls"; }

run 2 2
[ "$(count clone)" -eq 3 ]; check "a clone that fails twice is tried a third time" $?
[ "$(count 'swift package resolve')" -eq 3 ]; check "a resolve that fails twice is tried a third time" $?
[ "$(count generator)" -eq 1 ]; check "the generator runs once the fetches succeed" $?

run 3 0
[ "$(count clone)" -eq 3 ]; check "a clone that always fails is given up after three attempts" $?
[ "$(count generator)" -eq 0 ]; check "the generator never runs without its source" $?

run 0 3
[ "$(count 'swift package resolve')" -eq 3 ]; check "a resolve that always fails is given up after three attempts" $?
[ "$(count 'swift build -c')" -eq 0 ]; check "the generator is not built without its dependencies" $?

echo "generate-openapi retry: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
