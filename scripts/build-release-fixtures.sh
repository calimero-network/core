#!/usr/bin/env bash
# Builds the compute fixture (apps/v2-fixtures/compute, an app component and
# its compute component) from a release's own source, with that release's own
# tools (its meroctl; before meroctl built apps, its mero-app-build, mero-sign and
# scripts/pack-component.py): the package a
# cluster on that release would have installed. The release ships no compute
# fixture of its own (only kv-store-test-fixture.mpk, which fetch-release.sh
# downloads).
#
#   scripts/fetch-release.sh 1.0.0-rc.15 target/upgrade-from
#   scripts/build-release-fixtures.sh 1.0.0-rc.15 target/upgrade-from
#
# Writes OUT/compute.mpk (package com.calimero.compute-fixture, version TAG), and
# OUT/tools/bin/release-build and release-pack, the release's tools behind one
# spelling (`release-build CRATE --out F [--compute-out F]`, `release-pack --wasm F
# [--compute F] --package P --app-version V --out F`), which
# build-upgrade-deep-fixtures.sh reuses.
# Needs the wasm32-wasip2 target and OUT/meroctl (OUT/mero-sign for an older release:
# fetch-release.sh).
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
mkdir -p "$TOOLS/bin"
if [ -e "$SRC/scripts/pack-component.py" ]; then
  # A release from before meroctl built apps: its own mero-app-build (kept apart from
  # this tree's), its pack-component.py and the mero-sign it shipped.
  test -x "$OUT/mero-sign" || { echo "build-release-fixtures: $OUT/mero-sign missing (fetch-release.sh first)" >&2; exit 1; }
  CARGO_TARGET_DIR="$OUT/build" cargo install -q --locked --root "$TOOLS" --path "$SRC/crates/app-build"
  printf '#!/usr/bin/env bash\nexec %q "$@" --target-dir %q\n' "$TOOLS/bin/mero-app-build" "$OUT/build" >"$TOOLS/bin/release-build"
  # MEROCTL: a release whose pack-component.py is the shim over `meroctl app pack` runs
  # the release's own meroctl rather than building one.
  printf '#!/usr/bin/env bash\nMEROCTL=%q exec python3 -I %q --mero-sign %q "$@"\n' "$OUT/meroctl" "$SRC/scripts/pack-component.py" "$OUT/mero-sign" >"$TOOLS/bin/release-pack"
else
  # The release's meroctl builds, packs and signs (the development key, as the old tools did).
  test -x "$OUT/meroctl" || { echo "build-release-fixtures: $OUT/meroctl missing (fetch-release.sh first)" >&2; exit 1; }
  printf '#!/usr/bin/env bash\nexec %q app build "$@" --target-dir %q\n' "$OUT/meroctl" "$OUT/build" >"$TOOLS/bin/release-build"
  printf '#!/usr/bin/env bash\nexec %q app pack "$@"\n' "$OUT/meroctl" >"$TOOLS/bin/release-pack"
fi
chmod +x "$TOOLS/bin/release-build" "$TOOLS/bin/release-pack"

(cd "$SRC" && "$TOOLS/bin/release-build" apps/v2-fixtures/compute \
  --out "$OUT/compute-app.wasm" --compute-out "$OUT/compute-compute.wasm")
"$TOOLS/bin/release-pack" --wasm "$OUT/compute-app.wasm" --compute "$OUT/compute-compute.wasm" \
  --package com.calimero.compute-fixture --app-version "$TAG" --out "$OUT/compute.mpk"
echo "build-release-fixtures: $OUT/compute.mpk from $TAG ($(git -C "$SRC" rev-parse --short HEAD))" >&2
