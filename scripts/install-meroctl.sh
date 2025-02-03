#!/bin/bash

BINARY_NAME="meroctl"
REPO="calimero-network/core"
INSTALL_DIR="$HOME/.local/bin"

# Detect OS and Architecture
OS=$(uname | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$ARCH" in
  "x86_64") ARCH="x86_64" ;;
  "arm64" | "aarch64") ARCH="aarch64" ;;
  *)
    echo "Unsupported architecture: $ARCH."
    exit 1
    ;;
esac

if [ "$OS" == "darwin" ]; then
  PLATFORM="apple-darwin"
elif [ "$OS" == "linux" ]; then
  PLATFORM="unknown-linux-gnu"
else
  echo "Unsupported operating system: $OS."
  exit 1
fi

# Construct search URL
SEARCH_URL="https://api.github.com/repos/$REPO/releases?query=tag_name:$BINARY_NAME-*"

# Construct GraphQL query
QUERY='{"query": "query { repository(owner: \"calimero-network\", name: \"core\") { releases(last: 1, orderBy: {field: CREATED_AT, direction: DESC}, filterBy: {tagPrefix: \"meroctl-\"}) { nodes { tag_name } } } }"}'

# Fetch latest release tag
if [ -z "$1" ]; then
  VERSION=$(curl -s -X POST -H "Content-Type: application/json" -d "$QUERY" "https://api.github.com/graphql" | jq -r '.data.repository.releases.nodes[0].tag_name')
else
  VERSION="$BINARY_NAME-$1"
fi

echo "Release tag to install: $VERSION"

# Construct download URL
TARBALL_NAME="${BINARY_NAME}_${ARCH}-${PLATFORM}.tar.gz"
DOWNLOAD_URL="https://github.com/$REPO/releases/download/$VERSION/$TARBALL_NAME"

# Ensure installation directory exists
mkdir -p "$INSTALL_DIR"

# Download binary tarball
echo "Downloading $TARBALL_NAME from $DOWNLOAD_URL..."
curl -L -o "$TARBALL_NAME" "$DOWNLOAD_URL"

# Extract binary
echo "Extracting $TARBALL_NAME..."
tar -xzf "$TARBALL_NAME"
chmod +x "$BINARY_NAME"

# Move binary to user-local bin directory
mv "$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
rm "$TARBALL_NAME"

# Add $HOME/.local/bin to PATH if not already present
if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
  SHELL_CONFIG_FILE="$HOME/.bashrc"
  case "$SHELL" in
    */zsh) SHELL_CONFIG_FILE="$HOME/.zshrc" ;;
    */fish) SHELL_CONFIG_FILE="$HOME/.config/fish/config.fish" ;;
    */csh|*/tcsh) SHELL_CONFIG_FILE="$HOME/.cshrc" ;;
  esac

  echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$SHELL_CONFIG_FILE"
  echo "Added $HOME/.local/bin to PATH in $SHELL_CONFIG_FILE. Please reload your shell or run: source $SHELL_CONFIG_FILE"
fi

# Final message
echo "$BINARY_NAME installed successfully in $INSTALL_DIR."
echo "To verify the installation, make sure $INSTALL_DIR is in your PATH."
echo "Run the following command to update your current shell session if needed:"
echo "source <your-shell-config-file>"
echo "Then run '$BINARY_NAME --version' to confirm the installation."
