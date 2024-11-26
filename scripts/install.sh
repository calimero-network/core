#!/bin/bash

set -e

# Define version and repository
VERSION="v0.1.1"
REPO="calimero-network/core"
BINARY_NAME="meroctl"

# Detect OS
OS=$(uname | tr '[:upper:]' '[:lower:]')

# Detect Architecture
ARCH=$(uname -m)
case "$ARCH" in
  "x86_64") ARCH="x86_64" ;;
  "arm64" | "aarch64") ARCH="aarch64" ;;
  *)
    echo "Unsupported architecture: $ARCH."
    exit 1
    ;;
esac

# Determine platform
if [ "$OS" == "darwin" ]; then
  PLATFORM="apple-darwin"
elif [ "$OS" == "linux" ]; then
  PLATFORM="unknown-linux-gnu"
else
  echo "Unsupported operating system: $OS."
  exit 1
fi

# Construct download URL and tarball name
TARBALL_NAME="${BINARY_NAME}_${ARCH}-${PLATFORM}.tar.gz"
DOWNLOAD_URL="https://github.com/$REPO/releases/download/$VERSION/$TARBALL_NAME"

# Download binary tarball
echo "Downloading $TARBALL_NAME from $DOWNLOAD_URL..."
curl -L -o "$TARBALL_NAME" "$DOWNLOAD_URL"

# Extract tarball
echo "Extracting $TARBALL_NAME..."
tar -xzf "$TARBALL_NAME"

# Make binary executable
chmod +x "$BINARY_NAME"

# Move to /usr/local/bin (or another PATH directory)
INSTALL_DIR="/usr/local/bin"
if [ ! -w "$INSTALL_DIR" ]; then
  echo "You need sudo permissions to install to $INSTALL_DIR"
  sudo mv "$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
else
  mv "$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
fi

# Clean up tarball
rm "$TARBALL_NAME"

echo "$BINARY_NAME installed successfully! Run '$BINARY_NAME' to get started."
