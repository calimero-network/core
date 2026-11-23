#!/usr/bin/env bash
# scripts/fetch-release.sh on each host: the artifacts of the host's triple are fetched,
# and a host with no published triple is refused before any download.
# `uname`, `git` and `curl` are stand-ins on PATH, so no network is used.
#
#   bash scripts/tests/fetch-release-test.sh
set -euo pipefail

SCRIPT="$(cd "$(dirname "$0")/.." && pwd)/fetch-release.sh"
TAG=1.0.0-rc.1
TRIPLES="x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu aarch64-apple-darwin"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/release" "$TMP/bin" "$TMP/build"

for triple in $TRIPLES; do
  for tool in merod meroctl; do
    printf '#!/bin/sh\necho %s %s\n' "$tool" "$TAG" >"$TMP/build/$tool"
    chmod +x "$TMP/build/$tool"
    tar -czf "$TMP/release/${tool}_${triple}.tar.gz" -C "$TMP/build" "$tool"
  done
done
echo kv >"$TMP/release/kv-store-test-fixture.mpk"
(cd "$TMP/release" && sha256sum -- * >SHA256SUMS)

cat >"$TMP/bin/uname" <<'S'
#!/bin/sh
case "$1" in -s) echo "$FAKE_OS" ;; -m) echo "$FAKE_MACHINE" ;; esac
S
cat >"$TMP/bin/git" <<'S'
#!/bin/sh
exit 0
S
cat >"$TMP/bin/curl" <<'S'
#!/bin/sh
while [ $# -gt 1 ]; do [ "$1" = -o ] && dest=$2; shift; done
echo "$1" >>"$CURL_LOG"
cp "$RELEASE/$(basename "$1")" "$dest"
S
chmod +x "$TMP/bin/"*

fail=0
check() { if "$@"; then echo "ok   $CASE"; else echo "FAIL $CASE"; fail=1; fi; }

fetch() { # OS MACHINE: run the script as that host; sets CODE, and the URLs it fetched in $TMP/urls
  : >"$TMP/curl.log"
  rm -rf "$TMP/out"
  CODE=0
  PATH="$TMP/bin:$PATH" FAKE_OS=$1 FAKE_MACHINE=$2 CURL_LOG="$TMP/curl.log" RELEASE="$TMP/release" \
    bash "$SCRIPT" "$TAG" "$TMP/out" >/dev/null 2>"$TMP/err" || CODE=$?
}
fetched() { grep -q "/${1}_$2.tar.gz\$" "$TMP/curl.log"; }
fetched_only() { # TRIPLE: merod and meroctl of TRIPLE, and no other triple's
  fetched merod "$1" && fetched meroctl "$1" && [ "$(grep -c '\.tar\.gz$' "$TMP/curl.log")" = 2 ]
}

CASE="Linux x86_64 fetches the x86_64 Linux artifacts"
fetch Linux x86_64
check fetched_only x86_64-unknown-linux-gnu

CASE="Linux aarch64 fetches the aarch64 Linux artifacts"
fetch Linux aarch64
check fetched_only aarch64-unknown-linux-gnu

CASE="an Apple silicon Mac fetches the macOS artifacts"
fetch Darwin arm64
check fetched_only aarch64-apple-darwin
check [ "$CODE" = 0 ]

for host in "Darwin x86_64" "Linux riscv64" "FreeBSD amd64"; do
  CASE="$host is refused by name, with the supported triples, before any download"
  read -r os machine <<<"$host"
  fetch "$os" "$machine"
  check [ "$CODE" = 1 ]
  check grep -q "host $host" "$TMP/err"
  for triple in $TRIPLES; do check grep -q "$triple" "$TMP/err"; done
  check [ ! -s "$TMP/curl.log" ]
done

exit $fail
