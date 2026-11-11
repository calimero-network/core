#!/usr/bin/env bash
# Builds the compute fixture (apps/v2-fixtures/compute, an app component and
# its compute component) from a release's own source, with that release's
# mero-app-build, and signs it with the release's mero-sign: the package a
# cluster on that release would have installed. The release ships no compute
# fixture of its own (only kv-store-test-fixture.mpk, which fetch-release.sh
# downloads).
#
#   scripts/fetch-release.sh 1.0.0-rc.15 target/upgrade-from
#   scripts/build-release-fixtures.sh 1.0.0-rc.15 target/upgrade-from
#
# Writes OUT/compute.mpk (package com.calimero.compute-fixture, version TAG).
# Needs the wasm32-wasip2 target and OUT/mero-sign (fetch-release.sh).
set -euo pipefail

TAG="${1:?usage: build-release-fixtures.sh TAG OUT}"
OUT=$(mkdir -p "${2:?usage: build-release-fixtures.sh TAG OUT}" && cd "$2" && pwd -P)
REPO="${REPO:-calimero-network/core}"
SRC="$OUT/src"
TOOLS="$OUT/tools"

if [ ! -d "$SRC/.git" ]; then
  rm -rf "$SRC"
  git init -q "$SRC"
  git -C "$SRC" fetch -q --depth 1 "https://github.com/$REPO.git" "refs/tags/$TAG:refs/tags/$TAG"
  git -C "$SRC" checkout -q "refs/tags/$TAG"
fi
test -x "$OUT/mero-sign" || { echo "build-release-fixtures: $OUT/mero-sign missing (fetch-release.sh first)" >&2; exit 1; }

# The release's own mero-app-build, kept apart from this tree's.
CARGO_TARGET_DIR="$OUT/build" cargo install -q --locked --root "$TOOLS" --path "$SRC/crates/app-build"
(cd "$SRC" && "$TOOLS/bin/mero-app-build" apps/v2-fixtures/compute \
  --out "$OUT/compute-app.wasm" --compute-out "$OUT/compute-compute.wasm" --target-dir "$OUT/build")
python3 -I "$SRC/scripts/pack-component.py" --wasm "$OUT/compute-app.wasm" --compute "$OUT/compute-compute.wasm" \
  --mero-sign "$OUT/mero-sign" --package com.calimero.compute-fixture --app-version "$TAG" \
  --out "$OUT/compute.mpk"
echo "build-release-fixtures: $OUT/compute.mpk from $TAG ($(git -C "$SRC" rev-parse --short HEAD))" >&2
