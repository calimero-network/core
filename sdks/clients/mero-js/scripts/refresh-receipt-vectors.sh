#!/usr/bin/env bash
# Copies core's receipt vectors (crates/client/tests/fixtures/receipt-vectors.json,
# in this same tree) into src/client/fixtures/receipt-vectors.json, which
# src/client/receipts.test.ts checks mero-js's receipt verification against.
# Part of `pnpm generate`; never edit the copy by hand.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/../../../crates/client/tests/fixtures/receipt-vectors.json"

node -e 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"))' "$src" \
  || { echo "not JSON: $src" >&2; exit 1; }
cp "$src" "$root/src/client/fixtures/receipt-vectors.json"
echo "src/client/fixtures/receipt-vectors.json: crates/client/tests/fixtures/receipt-vectors.json"
