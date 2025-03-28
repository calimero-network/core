set -e

TAG=${CALIMERO_CONTRACTS_VERSION:-0.4.0}
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

RELEASE_DATA=$(curl -s "$API_URL")
ASSET_URLS=$(echo "$RELEASE_DATA" | jq -r '.assets[] | select(.name | endswith(".tar.gz")) | .browser_download_url')

if [ -z "$ASSET_URLS" ]; then
  echo "No .tar.gz assets found for ${TAG}"
  echo "API Response:"
  echo "$RELEASE_DATA" | jq '.'
  exit 1
fi

mkdir -p "$OUTPUT_DIR"

echo "$ASSET_URLS" | while read -r ASSET_URL; do
  ARTIFACT_NAME=$(basename "$ASSET_URL" .tar.gz)

  ARTIFACT_DIR="$OUTPUT_DIR/$ARTIFACT_NAME"

  mkdir -p "$ARTIFACT_DIR"

  echo "Downloading $ARTIFACT_NAME from $ASSET_URL..."
  curl -L "$ASSET_URL" -o "$ARTIFACT_DIR/artifact.tar.gz"

  echo "Extracting $ARTIFACT_NAME to $ARTIFACT_DIR..."
  tar -xzf "$ARTIFACT_DIR/artifact.tar.gz" -C "$ARTIFACT_DIR"

  rm "$ARTIFACT_DIR/artifact.tar.gz"
done

echo "All artifacts have been downloaded and extracted into $OUTPUT_DIR!"
