#!/usr/bin/env bash
# Vendors core's node API document (docs/public/openapi.json) at a core ref
# into openapi/core-openapi.json, records the commit in openapi/SOURCE, and
# regenerates src/generated/api.ts from it.
#
#   scripts/refresh-openapi.sh                    # the ref openapi/SOURCE names
#   scripts/refresh-openapi.sh master             # a branch, tag or commit of core
#   scripts/refresh-openapi.sh <sha> master       # that commit, recorded as on master
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
repo="calimero-network/core"
path="docs/public/openapi.json"
ref="${1:-$(sed -n 's/^ref=//p' "$root/openapi/SOURCE")}"
# The ref openapi/SOURCE records: the one given, else the one fetched (a
# commit sent by core's sync names the branch it was taken from this way).
recorded="${2:-$ref}"

if [[ "$ref" =~ ^[0-9a-f]{40}$ ]]; then
  commit="$ref"
else
  commit="$(git ls-remote "https://github.com/$repo" "$ref" "refs/heads/$ref" "refs/tags/$ref" | head -n1 | cut -f1)"
  [[ -n "$commit" ]] || { echo "no such ref in $repo: $ref" >&2; exit 1; }
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fsSL "https://raw.githubusercontent.com/$repo/$commit/$path" -o "$tmp"
node -e 'const d = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); if (!d.openapi || !d.paths) process.exit(1);' "$tmp" \
  || { echo "not an OpenAPI document: $repo@$commit:$path" >&2; exit 1; }
cp "$tmp" "$root/openapi/core-openapi.json"

cat > "$root/openapi/SOURCE" <<SOURCE
# The node API document src/generated/api.ts is generated from
# (\`pnpm gen:api\`), vendored from calimero-network/core.
# Refresh it with \`scripts/refresh-openapi.sh [<core ref>]\`.
repository=$repo
path=$path
ref=$recorded
commit=$commit
SOURCE

cd "$root" && pnpm gen:api
echo "openapi/core-openapi.json: $repo@$commit ($recorded)"
