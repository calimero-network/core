#!/bin/bash
set -euo pipefail

CHANGED_PATH="${1:-}"

if [ -z "$CHANGED_PATH" ]; then
  echo "sync-wasm: usage: sync-wasm.sh <path-to-wasm>"
  exit 1
fi

if [ ! -f "$CHANGED_PATH" ]; then
  echo "sync-wasm: file not found: $CHANGED_PATH"
  exit 1
fi

NAME=$(basename "$CHANGED_PATH")

echo "[sync-wasm] copying $CHANGED_PATH to data nodes as $NAME"
cp "$CHANGED_PATH" "data/calimero-node-1/$NAME"
cp "$CHANGED_PATH" "data/calimero-node-2/$NAME"