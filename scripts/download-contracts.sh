#!/usr/bin/env bash
set -euo pipefail

TAG=${CALIMERO_CONTRACTS_VERSION:-0.7.0}
OUTPUT_DIR=${CALIMERO_CONTRACTS_DIR:-contracts}

REPO_OWNER="calimero-network"
REPO_NAME="contracts"

if [ "$TAG" = "latest" ]; then
  echo "Fetching latest release..."
  API_URL="https://api.github.com/repos/$REPO_OWNER/$REPO_NAME/releases/latest"
else
  echo "Fetching release for tag: $TAG"
  API_URL="https://api.github.com/repos/$REPO_OWNER/$REPO_NAME/releases/tags/$TAG"
fi

RELEASE_DATA=$(curl --fail --silent --show-error \
  -H "Accept: application/vnd.github+json" "$API_URL")

# Emit "<download-url>\t<sha256-digest>" per .tar.gz asset. The digest is the
# hash GitHub computed for the stored asset and returns over TLS from the API;
# we verify each download against it before extracting so a tampered or
# truncated archive from the download CDN is rejected.
ASSETS=$(echo "$RELEASE_DATA" \
  | jq -r '.assets[] | select(.name | endswith(".tar.gz")) | [.browser_download_url, .digest] | @tsv')

if [ -z "$ASSETS" ]; then
  echo "No .tar.gz assets found for ${TAG}"
  echo "API Response:"
  echo "$RELEASE_DATA" | jq '.'
  exit 1
fi

mkdir -p "$OUTPUT_DIR"

while IFS=$'\t' read -r ASSET_URL ASSET_DIGEST; do
  [ -n "$ASSET_URL" ] || continue

  ARTIFACT_NAME=$(basename "$ASSET_URL" .tar.gz)
  ARTIFACT_DIR="$OUTPUT_DIR/$ARTIFACT_NAME"
  TARBALL="$ARTIFACT_DIR/artifact.tar.gz"

  mkdir -p "$ARTIFACT_DIR"

  echo "Downloading $ARTIFACT_NAME from $ASSET_URL..."
  curl --fail --silent --show-error --location "$ASSET_URL" -o "$TARBALL"

  EXPECTED="${ASSET_DIGEST#sha256:}"
  if [ -z "$EXPECTED" ] || [ "$EXPECTED" = "null" ]; then
    echo "ERROR: no sha256 digest published for $ARTIFACT_NAME; refusing to extract" >&2
    exit 1
  fi

  ACTUAL=$(sha256sum "$TARBALL" | awk '{print $1}')
  if [ "$ACTUAL" != "$EXPECTED" ]; then
    echo "ERROR: checksum mismatch for $ARTIFACT_NAME" >&2
    echo "  expected: $EXPECTED" >&2
    echo "  actual:   $ACTUAL" >&2
    exit 1
  fi
  echo "Checksum verified for $ARTIFACT_NAME"

  echo "Extracting $ARTIFACT_NAME to $ARTIFACT_DIR..."
  tar -xzf "$TARBALL" -C "$ARTIFACT_DIR"

  rm "$TARBALL"
done <<< "$ASSETS"

echo "All artifacts have been downloaded and extracted into $OUTPUT_DIR!"
