#!/usr/bin/env bash
# Fetches a published core release's Linux x86_64 artifacts, the ones a
# Calimero Cloud node runs: merod, meroctl and mero-sign from the GitHub
# release's tarballs, checked against its SHA256SUMS, and the release's
# dev-signed kv fixture (kv-store-test-fixture.mpk).
#
#   scripts/fetch-release.sh 1.0.0-rc.15 target/upgrade-from
#   scripts/fetch-release.sh "$(cat .github/upgrade-from)" target/upgrade-from
#
# Lays out OUT/{merod,meroctl,mero-sign,kv-store-test-fixture.mpk,SHA256SUMS,VERSION}.
# Exit status: 0 fetched; 3 the tag does not exist (yet); 1 anything else,
# including a tag whose release lacks an artifact (never a silent pass).
# REPO (default calimero-network/core) names another repository.
set -euo pipefail

TAG="${1:?usage: fetch-release.sh TAG OUT}"
OUT="${2:?usage: fetch-release.sh TAG OUT}"
REPO="${REPO:-calimero-network/core}"
TRIPLE=x86_64-unknown-linux-gnu
BASE="https://github.com/$REPO/releases/download/$TAG"

if ! git ls-remote --exit-code --tags "https://github.com/$REPO.git" "refs/tags/$TAG" >/dev/null; then
  echo "fetch-release: $REPO has no tag $TAG" >&2
  exit 3
fi

mkdir -p "$OUT"
DL=$(mktemp -d)
trap 'rm -rf "$DL"' EXIT

fetch() {
  if ! curl -fsSL --retry 3 --retry-delay 5 -o "$DL/$1" "$BASE/$1"; then
    echo "::error::fetch-release: release $TAG exists but $1 could not be downloaded from $BASE/$1." \
      "The release workflow (release.yml) publishes it; re-run that workflow for $TAG, or fix the asset." >&2
    exit 1
  fi
}

fetch SHA256SUMS
for bin in merod meroctl mero-sign; do
  fetch "${bin}_${TRIPLE}.tar.gz"
done
fetch kv-store-test-fixture.mpk

(cd "$DL" && sha256sum --check --ignore-missing --strict SHA256SUMS) >&2
for bin in merod meroctl mero-sign; do
  if ! grep -q " ${bin}_${TRIPLE}.tar.gz\$" "$DL/SHA256SUMS"; then
    echo "::error::fetch-release: SHA256SUMS of $TAG does not list ${bin}_${TRIPLE}.tar.gz" >&2
    exit 1
  fi
  tar -xzf "$DL/${bin}_${TRIPLE}.tar.gz" -C "$OUT" "$bin"
  chmod +x "$OUT/$bin"
done
cp "$DL/kv-store-test-fixture.mpk" "$DL/SHA256SUMS" "$OUT/"

got=$("$OUT/merod" --version | awk '{print $2}')
if [ "$got" != "$TAG" ]; then
  echo "::error::fetch-release: the merod of release $TAG reports version $got" >&2
  exit 1
fi
printf '%s\n' "$TAG" >"$OUT/VERSION"
echo "fetch-release: $TAG -> $OUT ($("$OUT/merod" --version), $("$OUT/meroctl" --version))" >&2
