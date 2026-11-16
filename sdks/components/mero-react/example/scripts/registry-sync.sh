#!/bin/bash
# Sync WASM to local registry (uses file:// URIs)
set -euo pipefail

WASM_FILE="${1:-logic/res/kv_store.wasm}"
MANIFEST_FILE="${2:-manifest.json}"
REGISTRY_URL="${REGISTRY_URL:-http://localhost:8082}"

if [ ! -f "$WASM_FILE" ]; then
  echo "⚠️  WASM file not found: $WASM_FILE"
  exit 0
fi

# Check if registry is running
if ! curl -sf "$REGISTRY_URL/healthz" > /dev/null 2>&1; then
  echo "⚠️  Local registry not running (skipping sync)"
  echo "   Start it: pnpm registry:start"
  exit 0
fi

# Calculate digest
DIGEST=$(shasum -a 256 "$WASM_FILE" | awk '{print $1}')

# Get absolute path for file:// URI
ABS_WASM_PATH="$(cd "$(dirname "$WASM_FILE")" && pwd)/$(basename "$WASM_FILE")"

echo "📤 Syncing to local registry..."
echo "   File: $(basename "$WASM_FILE")"
echo "   Digest: ${DIGEST:0:16}..."

# Create temporary manifest with file:// URI
TEMP_MANIFEST=$(mktemp)
trap "rm -f $TEMP_MANIFEST" EXIT

jq --arg digest "sha256:$DIGEST" \
   --arg uri "file://$ABS_WASM_PATH" \
   '.artifact.digest = $digest | .artifact.uri = $uri' \
   "$MANIFEST_FILE" > "$TEMP_MANIFEST"

# Submit to registry
RESPONSE=$(curl -sf -X POST "$REGISTRY_URL/v1/apps" \
  -H "Content-Type: application/json" \
  -d @"$TEMP_MANIFEST" 2>&1)

if echo "$RESPONSE" | jq -e '.id' > /dev/null 2>&1; then
  APP_ID=$(echo "$RESPONSE" | jq -r '.id')
  VERSION=$(echo "$RESPONSE" | jq -r '.version')
  echo "✅ Synced: $APP_ID@$VERSION"
  echo "   Registry will serve via: $REGISTRY_URL/artifacts/$APP_ID/$VERSION/$(basename "$WASM_FILE")"
else
  # Might be duplicate - that's OK
  if echo "$RESPONSE" | jq -e '.error' 2>/dev/null | grep -q "already_exists"; then
    echo "✅ Already synced (version exists)"
  else
    echo "❌ Registry error:"
    echo "$RESPONSE" | jq '.' 2>/dev/null || echo "$RESPONSE"
  fi
fi


