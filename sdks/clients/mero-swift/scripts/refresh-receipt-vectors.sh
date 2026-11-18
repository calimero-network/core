#!/usr/bin/env bash
# Copies core's receipt vectors (crates/client/tests/fixtures/receipt-vectors.json,
# in the core checkout this SDK sits in) into
# Tests/MeroKitTests/Fixtures/receipts/receipt-vectors.json, which
# ReceiptTests checks MeroKit's receipt verification against. Part of
# sdk.json's `generate`; never edit the copy by hand.
#
#   scripts/refresh-receipt-vectors.sh          copy them
#   scripts/refresh-receipt-vectors.sh check    exit 1 if the copy differs
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/../../../crates/client/tests/fixtures/receipt-vectors.json"
DEST="$ROOT/Tests/MeroKitTests/Fixtures/receipts/receipt-vectors.json"

[ -f "$SRC" ] || { echo "refresh-receipt-vectors: no vectors at $SRC: run this inside calimero-network/core" >&2; exit 1; }
python3 -c 'import json, sys; json.load(open(sys.argv[1]))' "$SRC" || { echo "not JSON: $SRC" >&2; exit 1; }

case "${1:-generate}" in
  generate)
    mkdir -p "$(dirname "$DEST")"
    cp "$SRC" "$DEST"
    echo "Tests/MeroKitTests/Fixtures/receipts/receipt-vectors.json: crates/client/tests/fixtures/receipt-vectors.json"
    ;;
  check)
    cmp -s "$SRC" "$DEST" || { echo "refresh-receipt-vectors: the copy is stale: run scripts/refresh-receipt-vectors.sh" >&2; exit 1; }
    echo "the receipt vectors match crates/client/tests/fixtures/receipt-vectors.json"
    ;;
  *)
    sed -n '2,10p' "$0" >&2
    exit 2
    ;;
esac
