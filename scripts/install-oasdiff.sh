#!/usr/bin/env bash
# Puts oasdiff, which scripts/openapi-classify.py diffs OpenAPI documents with, at
# DIR/oasdiff (default: $RUNNER_TEMP/oasdiff, else under $TMPDIR or /tmp), checked
# against the release's SHA-256, and prints that path.
#
#   scripts/install-oasdiff.sh [DIR]
#
# Pinned: a new oasdiff can reclassify a change, so a bump is reviewed like code
# (run scripts/tests/openapi-classify-test.py with it). The checksums are the
# release's checksums.txt entries.
set -euo pipefail

VERSION=1.37.0
dir="${1:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/oasdiff}"

case "$(uname -s)/$(uname -m)" in
  Linux/x86_64) asset=linux_amd64 sha=eac87307d31d08c36800c58a409c247f1c5db37fbb72e2544402f22a87cbca3c ;;
  Linux/aarch64 | Linux/arm64) asset=linux_arm64 sha=072a425ac647ecf2808721e91293fd644cee7935d2b32e650b3158539b2c7898 ;;
  Darwin/*) asset=darwin_all sha=eccf5b9f46e689f8adcd79b79d4fdbde26a28301b6a74a0b449ad6a49e187e45 ;;
  *) echo "no oasdiff release for $(uname -s)/$(uname -m)" >&2; exit 1 ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/oasdiff.tar.gz" \
  "https://github.com/oasdiff/oasdiff/releases/download/v${VERSION}/oasdiff_${VERSION}_${asset}.tar.gz"
if command -v sha256sum >/dev/null; then
  echo "${sha}  $tmp/oasdiff.tar.gz" | sha256sum -c - >/dev/null
else
  echo "${sha}  $tmp/oasdiff.tar.gz" | shasum -a 256 -c - >/dev/null
fi
tar -xzf "$tmp/oasdiff.tar.gz" -C "$tmp" oasdiff
mkdir -p "$dir"
install -m 0755 "$tmp/oasdiff" "$dir/oasdiff"
"$dir/oasdiff" --version >&2
echo "$dir/oasdiff"
