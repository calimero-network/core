#!/usr/bin/env bash
# Builds the deep upgrade cases' fixtures (crates/node/tests/fault/upgrade_cases/20_*.py
# and on) from a release's own source, with that release's own tools: the
# components and packages a cluster on that release would run. Run after
# fetch-release.sh and build-release-fixtures.sh, whose checkout (OUT/src) and
# tools (OUT/tools/bin/release-build and release-pack) it reuses.
#
#   scripts/fetch-release.sh 1.0.0-rc.15 target/upgrade-from
#   scripts/build-release-fixtures.sh 1.0.0-rc.15 target/upgrade-from
#   scripts/build-upgrade-deep-fixtures.sh target/upgrade-from
#
# Writes OUT/deep/: kv-dev.wasm (the release's kv fixture component from its
# kv-store-test-fixture.mpk, as a dev bundle of its own), files.wasm, and the signed
# packages kv-a-1.0.0.mpk, kv-a-1.1.0.mpk, kv-b-1.0.0.mpk, kv-c-1.0.0.mpk,
# kv-u-1.0.0.mpk, kv-u-1.1.0.mpk (com.calimero.upgrade.kv-*), notes-1.0.0.mpk and notes-2.0.0.mpk
# (com.calimero.upgrade.notes, apps/v2-fixtures/notes-v1 and notes-v2).
set -euo pipefail

OUT=$(cd "${1:?usage: build-upgrade-deep-fixtures.sh OUT}" && pwd -P)
SRC="$OUT/src"
BUILD="$OUT/tools/bin/release-build"
PACK="$OUT/tools/bin/release-pack"
DEEP="$OUT/deep"
for need in "$SRC/.git" "$BUILD" "$PACK" "$OUT/kv-store-test-fixture.mpk"; do
  test -e "$need" || { echo "build-upgrade-deep-fixtures: $need missing (fetch-release.sh and build-release-fixtures.sh first)" >&2; exit 1; }
done
mkdir -p "$DEEP"

# The release's kv component, as components of their own (one custom section each):
# kv-dev.wasm, every account's dev bundle, and one per package below
# (scripts/upgrade-dev-bundle.py says why).
python3 -I "$(dirname "$0")/upgrade-dev-bundle.py" "$OUT/kv-store-test-fixture.mpk" "$DEEP" dev a b c u

for app in files notes-v1 notes-v2; do
  (cd "$SRC" && "$BUILD" "apps/v2-fixtures/$app" --out "$DEEP/$app.wasm")
done

pack() { # WASM PACKAGE VERSION OUT
  "$PACK" --wasm "$1" --package "$2" --app-version "$3" --out "$DEEP/$4"
}
pack "$DEEP/kv-a.wasm" com.calimero.upgrade.kv-a 1.0.0 kv-a-1.0.0.mpk
pack "$DEEP/kv-a.wasm" com.calimero.upgrade.kv-a 1.1.0 kv-a-1.1.0.mpk
pack "$DEEP/kv-b.wasm" com.calimero.upgrade.kv-b 1.0.0 kv-b-1.0.0.mpk
pack "$DEEP/kv-c.wasm" com.calimero.upgrade.kv-c 1.0.0 kv-c-1.0.0.mpk
pack "$DEEP/kv-u.wasm" com.calimero.upgrade.kv-u 1.0.0 kv-u-1.0.0.mpk
pack "$DEEP/kv-u.wasm" com.calimero.upgrade.kv-u 1.1.0 kv-u-1.1.0.mpk
pack "$DEEP/notes-v1.wasm" com.calimero.upgrade.notes 1.0.0 notes-1.0.0.mpk
pack "$DEEP/notes-v2.wasm" com.calimero.upgrade.notes 2.0.0 notes-2.0.0.mpk
echo "build-upgrade-deep-fixtures: $DEEP from $(git -C "$SRC" describe --tags --always)" >&2
ls -l "$DEEP" >&2
