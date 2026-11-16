#!/usr/bin/env bash
#
# Tests for wait-for-npm.sh against a stub `npm`. No network.
#
#   bash .github/scripts/tests/wait-for-npm.test.sh

set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd -P)
WAIT="$HERE/../wait-for-npm.sh"
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  ok    %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL  %s\n' "$1"; }

# A stub npm that 404s until it has been asked $SERVE_AFTER times, then serves $SERVED.
mkdir -p "$ROOT/bin"
cat > "$ROOT/bin/npm" <<'EOF'
#!/usr/bin/env bash
n=$(( $(cat "$COUNT_FILE" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$COUNT_FILE"
if [ "$n" -gt "$SERVE_AFTER" ]; then echo "$SERVED"; else echo "npm error 404" >&2; exit 1; fi
EOF
chmod +x "$ROOT/bin/npm"

run() { # serve_after served -> exit status of the wait
  rm -f "$ROOT/count"
  PATH="$ROOT/bin:$PATH" COUNT_FILE="$ROOT/count" SERVE_AFTER=$1 SERVED=$2 \
    WAIT_TRIES=3 WAIT_SLEEP_S=0 "$WAIT" @scope/pkg=1.2.3 >/dev/null 2>&1
}

expect() { # want serve_after served name
  run "$2" "$3"
  if [ $? -eq "$1" ]; then ok "$4"; else bad "$4"; fi
}

expect 0 0 1.2.3 "already served"
expect 0 2 1.2.3 "served after two 404s"
expect 1 5 1.2.3 "never served within the tries fails"
expect 1 0 1.2.2 "a different version is not the one asked for"

echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
