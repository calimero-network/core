#!/usr/bin/env bash
# Vendors core's receipt vectors (crates/client/tests/fixtures/receipt-vectors.json)
# at a core ref into src/client/fixtures/receipt-vectors.json.
#
#   scripts/refresh-receipt-vectors.sh            # master
#   scripts/refresh-receipt-vectors.sh <core ref> # a branch, tag or commit of core
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
repo="calimero-network/core"
path="crates/client/tests/fixtures/receipt-vectors.json"
ref="${1:-master}"

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fsSL "https://raw.githubusercontent.com/$repo/$ref/$path" -o "$tmp"
node -e 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"))' "$tmp" \
  || { echo "not JSON: $repo@$ref:$path" >&2; exit 1; }
cp "$tmp" "$root/src/client/fixtures/receipt-vectors.json"
echo "src/client/fixtures/receipt-vectors.json: $repo@$ref"
