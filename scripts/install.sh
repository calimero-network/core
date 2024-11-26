#!/bin/bash

BINARY_NAME="meroctl"
VERSION="v0.1.1"
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
if ! echo "$PATH" | grep -q "$HOME/.local/bin"; then
  echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.bashrc"
  echo "Added $HOME/.local/bin to PATH. Reload your shell or run: source ~/.bashrc"
fi

echo "$BINARY_NAME installed successfully in $INSTALL_DIR. Run '$BINARY_NAME --version' to verify."
