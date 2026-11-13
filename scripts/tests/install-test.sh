#!/usr/bin/env bash
# scripts/install.sh against a release laid out on disk (file:// URLs): the
# checksum is checked, the newest release is found, and a bad request fails.
#
#   bash scripts/tests/install-test.sh
set -euo pipefail

SCRIPT="$(cd "$(dirname "$0")/.." && pwd)/install.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

case "$(uname -m)" in x86_64|amd64) ARCH=x86_64 ;; *) ARCH=aarch64 ;; esac
case "$(uname -s)" in Linux) TARGET="$ARCH-unknown-linux-gnu" ;; *) TARGET=aarch64-apple-darwin ;; esac

release() { # VERSION: a release with fake merod and meroctl archives and SHA256SUMS
  local dir="$TMP/releases/download/$1"
  mkdir -p "$dir" "$TMP/build"
  for tool in merod meroctl; do
    printf '#!/bin/sh\necho %s %s\n' "$tool" "$1" > "$TMP/build/$tool"
    chmod +x "$TMP/build/$tool"
    tar -czf "$dir/${tool}_${TARGET}.tar.gz" -C "$TMP/build" "$tool"
  done
  (cd "$dir" && sha256sum -- * > SHA256SUMS)
}
release 1.0.0-rc.1
release 1.0.0-rc.2
mkdir -p "$TMP/api"
printf '[{"tag_name": "1.0.0-rc.2", "prerelease": true}]' > "$TMP/api/releases"

run() {
  CALIMERO_RELEASES="file://$TMP/releases" CALIMERO_RELEASES_API="file://$TMP/api/releases" \
    sh "$SCRIPT" --dir "$TMP/bin" "$@"
}
fail=0
check() { if "$@"; then echo "ok   $CASE"; else echo "FAIL $CASE"; fail=1; fi; }

CASE="meroctl by default, the newest release"
run >/dev/null 2>&1
check [ "$("$TMP/bin/meroctl")" = "meroctl 1.0.0-rc.2" ]

CASE="a named version"
run meroctl --version 1.0.0-rc.1 >/dev/null 2>&1
check [ "$("$TMP/bin/meroctl")" = "meroctl 1.0.0-rc.1" ]

if [ "$(uname -s)" = Linux ]; then
  CASE="merod and meroctl together"
  run merod meroctl --version 1.0.0-rc.2 >/dev/null 2>&1
  check [ "$("$TMP/bin/merod")" = "merod 1.0.0-rc.2" ]
fi

CASE="an archive that does not match SHA256SUMS is refused and not installed"
rm -f "$TMP/bin/meroctl"
printf 'tampered' >> "$TMP/releases/download/1.0.0-rc.1/meroctl_${TARGET}.tar.gz"
if run meroctl --version 1.0.0-rc.1 2>"$TMP/err" >/dev/null; then echo "FAIL tampered archive installed"; fail=1; fi
check grep -q "does not match SHA256SUMS" "$TMP/err"
check [ ! -e "$TMP/bin/meroctl" ]

CASE="an unknown version fails with a hint"
if run --version 9.9.9 2>"$TMP/err" >/dev/null; then fail=1; fi
check grep -q "has no SHA256SUMS" "$TMP/err"

CASE="an unknown tool is refused"
if run mero-app-build 2>"$TMP/err" >/dev/null; then fail=1; fi
check grep -q "unknown argument" "$TMP/err"

exit "$fail"
