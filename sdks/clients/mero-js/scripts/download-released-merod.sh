#!/usr/bin/env bash
# Download a released Calimero 1.0 merod for linux-x86_64 into ./merod, with the
# release's kv-store test fixture as ./kv_store.wasm, for the cluster tests
# (`pnpm test:cluster` with MEROD_BINARY and KV_STORE_WASM).
#
# Only releases of one major line are considered (MEROD_LINE, default 1): the
# 0.x releases are v1 nodes, which the 1.0 client cannot talk to, and a future
# 2.0 should be a deliberate move, not whatever happens to be newest. Prereleases
# count (1.0.0-rc.1 is the first 1.0 release). MEROD_TAG names one release
# instead.
#
# Walks back through the line's releases rather than taking only the newest:
# GitHub publishes a release object before its binary matrix finishes uploading,
# so the newest tag can carry no merod yet while the one before it is usable.
#
# The binary is checked against the release's SHA256SUMS. The fixture is not
# listed there; its app.wasm is checked against the digest its signed manifest
# names.
set -euo pipefail

REPO="calimero-network/core"                    # where merod is released
ASSET="merod_x86_64-unknown-linux-gnu.tar.gz"   # the CI runner's platform
FIXTURE="kv-store-test-fixture.mpk"             # signed kv-store, built with the release
LINE="${MEROD_LINE:-1}"                         # major line to take releases from
LOOKBACK=30                                     # releases to walk back through

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Newest first, as the API lists them. `gh api` (REST), not `gh release list`.
if [ -n "${MEROD_TAG:-}" ]; then
  tags="$MEROD_TAG"
else
  tags=$(gh api "repos/$REPO/releases?per_page=$LOOKBACK" -q '.[] | select(.draft | not) | .tag_name' |
    grep -E "^v?${LINE}\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$" || true)
fi
[ -n "$tags" ] || { echo "::error::no ${LINE}.x release of $REPO in the last $LOOKBACK" >&2; exit 1; }

for tag in $tags; do
  assets=$(gh api "repos/$REPO/releases/tags/$tag" -q '.assets[] | .name + " " + .browser_download_url')
  url() { awk -v n="$1" '$1 == n { print $2 }' <<<"$assets"; }
  if [ -z "$(url "$ASSET")" ] || [ -z "$(url SHA256SUMS)" ] || [ -z "$(url "$FIXTURE")" ]; then
    echo "release $tag lacks $ASSET, SHA256SUMS or $FIXTURE; trying the one before it"
    continue
  fi
  echo "Downloading merod from $tag"
  for name in "$ASSET" SHA256SUMS "$FIXTURE"; do
    curl -fsSL --retry 3 -o "$work/$name" "$(url "$name")"
  done
  grep -qE "  $ASSET\$" "$work/SHA256SUMS" || { echo "::error::$tag's SHA256SUMS does not list $ASSET" >&2; exit 1; }
  (cd "$work" && sha256sum -c --ignore-missing SHA256SUMS)

  mkdir "$work/merod" "$work/fixture"
  tar -xzf "$work/$ASSET" -C "$work/merod" merod
  tar -xzf "$work/$FIXTURE" -C "$work/fixture" manifest.json app.wasm
  want=$(jq -r .wasm.hash "$work/fixture/manifest.json")
  have=$(sha256sum "$work/fixture/app.wasm" | cut -d' ' -f1)
  [ "$want" = "$have" ] || { echo "::error::$FIXTURE's app.wasm is not the wasm its manifest names" >&2; exit 1; }

  "$work/merod/merod" run --help > "$work/help"
  grep -q -- '--public-url' "$work/help" ||
    { echo "::error::merod from $tag does not take 1.0's flags" >&2; exit 1; }
  install -m 0755 "$work/merod/merod" ./merod
  install -m 0644 "$work/fixture/app.wasm" ./kv_store.wasm
  echo "merod $tag ready: ./merod, ./kv_store.wasm"
  if [ -n "${GITHUB_OUTPUT:-}" ]; then echo "tag=$tag" >> "$GITHUB_OUTPUT"; fi
  exit 0
done

echo "::error::no ${LINE}.x release of $REPO carries $ASSET, SHA256SUMS and $FIXTURE" >&2
exit 1
