#!/usr/bin/env bash
#
# refresh-openapi.sh — vendor core's OpenAPI document and regenerate the client from it.
#
# Usage: scripts/refresh-openapi.sh [ref]      (default ref: master)
#        scripts/refresh-openapi.sh --check    (regenerate from the vendored copy; fail on a diff)
#
# Reads docs/public/openapi.json at <ref> of calimero-network/core (a branch, tag or commit),
# writes it to openapi/openapi.json, records the commit it resolved to in openapi/SOURCE, then
# runs the standalone codegen build (codegen/), which rewrites mero-core/src/generated/kotlin.
# Review and commit both: the generated sources are checked in so the Android build needs
# neither the generator nor the network (see openapi/README.md).
#
# Needs `gh` (authenticated) or plain `git` with access to the core repository.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="calimero-network/core"
SPEC_PATH="docs/public/openapi.json"

generate() {
  "$ROOT/gradlew" -p "$ROOT/codegen" --no-daemon generateMeroClient
}

if [[ "${1:-}" == "--check" ]]; then
  generate
  if ! git -C "$ROOT" diff --exit-code -- mero-core/src/generated; then
    echo "mero-core/src/generated is stale: run scripts/refresh-openapi.sh --check and commit the result" >&2
    exit 1
  fi
  if [[ -n "$(git -C "$ROOT" status --porcelain -- mero-core/src/generated)" ]]; then
    git -C "$ROOT" status --porcelain -- mero-core/src/generated >&2
    echo "mero-core/src/generated has files the generator did not write, or misses some it did" >&2
    exit 1
  fi
  exit 0
fi

REF="${1:-master}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
  COMMIT="$(gh api "repos/$REPO/commits/$REF" --jq .sha)"
  gh api -H "Accept: application/vnd.github.raw" "repos/$REPO/contents/$SPEC_PATH?ref=$COMMIT" > "$TMP/openapi.json"
else
  git -C "$TMP" init -q
  git -C "$TMP" fetch -q --depth 1 "https://github.com/$REPO.git" "$REF"
  COMMIT="$(git -C "$TMP" rev-parse FETCH_HEAD)"
  git -C "$TMP" show "FETCH_HEAD:$SPEC_PATH" > "$TMP/openapi.json"
fi

# A truncated or HTML answer must not replace the spec.
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["openapi"].startswith("3.") and d["paths"]' "$TMP/openapi.json"

cp "$TMP/openapi.json" "$ROOT/openapi/openapi.json"
SHA="$(sha256sum "$ROOT/openapi/openapi.json" | cut -d' ' -f1)"
cat > "$ROOT/openapi/SOURCE" <<EOF
# Where openapi.json came from. Written by scripts/refresh-openapi.sh; do not edit by hand.
CORE_REPO=$REPO
CORE_REF=$REF
CORE_COMMIT=$COMMIT
CORE_PATH=$SPEC_PATH
SHA256=$SHA
EOF

generate
echo "openapi.json from $REPO@$COMMIT ($REF); review: git diff -- openapi mero-core/src/generated"
