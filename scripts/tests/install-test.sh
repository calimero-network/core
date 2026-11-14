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

# A Mac. 1.0.0-rc.2 has meroctl for it and no merod (as before 1.0.0-rc.25);
# 1.0.0-rc.1 has both.
mac_release() { # VERSION TOOLS...: add macOS archives of TOOLS to VERSION
  local version=$1; shift
  for tool in "$@"; do
    printf '#!/bin/sh\necho %s mac %s\n' "$tool" "$version" > "$TMP/build/$tool" && chmod +x "$TMP/build/$tool"
    tar -czf "$TMP/releases/download/$version/${tool}_aarch64-apple-darwin.tar.gz" -C "$TMP/build" "$tool"
  done
  (cd "$TMP/releases/download/$version" && sha256sum -- * > SHA256SUMS)
}
mac_release 1.0.0-rc.2 meroctl
mac_release 1.0.0-rc.1 merod meroctl
mac() {
  CALIMERO_RELEASES="file://$TMP/releases" CALIMERO_RELEASES_API="file://$TMP/api/releases" \
    CALIMERO_INSTALL_OS=Darwin CALIMERO_INSTALL_MACHINE=arm64 sh "$SCRIPT" "$@"
}

CASE="on macOS, a release with a macOS merod installs both"
mkdir -p "$TMP/mac-new"
if mac --dir "$TMP/mac-new" merod meroctl --version 1.0.0-rc.1 >/dev/null 2>"$TMP/err"; then :; else fail=1; fi
check [ "$("$TMP/mac-new/merod")" = "merod mac 1.0.0-rc.1" ]
check [ "$("$TMP/mac-new/meroctl")" = "meroctl mac 1.0.0-rc.1" ]

CASE="on macOS, a release with no macOS merod installs meroctl and skips merod with a note"
mkdir -p "$TMP/mac-old"
if mac --dir "$TMP/mac-old" merod meroctl --version 1.0.0-rc.2 >/dev/null 2>"$TMP/err"; then :; else fail=1; fi
check [ "$("$TMP/mac-old/meroctl")" = "meroctl mac 1.0.0-rc.2" ]
check [ ! -e "$TMP/mac-old/merod" ]
check grep -q "skipping merod" "$TMP/err"

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
