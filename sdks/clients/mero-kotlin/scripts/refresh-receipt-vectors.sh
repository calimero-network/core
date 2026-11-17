#!/usr/bin/env bash
# Copies core's receipt vectors (crates/client/tests/fixtures/receipt-vectors.json,
# in this same tree) into mero-core/src/test/resources/fixtures/receipt-vectors.json,
# which ReceiptsTest checks mero-kotlin's receipt verification against.
# Part of `sdk.json` `generate`; never edit the copy by hand.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/../../../crates/client/tests/fixtures/receipt-vectors.json"
dst="$root/mero-core/src/test/resources/fixtures/receipt-vectors.json"

python3 -c 'import json, sys; json.load(open(sys.argv[1]))' "$src" \
  || { echo "not JSON: $src" >&2; exit 1; }
cp "$src" "$dst"
echo "mero-core/src/test/resources/fixtures/receipt-vectors.json: crates/client/tests/fixtures/receipt-vectors.json"
