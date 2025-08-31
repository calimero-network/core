#!/bin/bash

# Local script to run the publishing workflow
set -e

echo "🚀 Running Calimero Crates Publishing Workflow Locally"

# Check if CARGO_REGISTRY_TOKEN is set
if [ -z "$CARGO_REGISTRY_TOKEN" ]; then
    echo "❌ Error: CARGO_REGISTRY_TOKEN environment variable is not set"
    echo "Please set it with: export CARGO_REGISTRY_TOKEN='your_token_here'"
    exit 1
fi

# Check if jq is installed
if ! command -v jq &> /dev/null; then
    echo "❌ Error: jq is not installed"
    echo "Please install it: brew install jq (macOS) or sudo apt install jq (Ubuntu)"
    exit 1
fi

# Check Rust version
echo "🔧 Checking Rust installation..."
rustc --version

# Check if version is already published
echo "📋 Checking if version is already published..."
PACKAGE_NAME="calimero-primitives"
VERSION=$(cargo metadata --no-deps --format-version 1 | jq -r '.metadata.workspaces.version // empty' 2>/dev/null || echo "")

if [ -z "$VERSION" ]; then
    echo "❌ ERROR: Failed to extract version. Exiting."
    exit 1
fi

echo "📦 Publishing version: $VERSION"

# Check if version is already published
CRATES_RESPONSE=$(curl -s "https://crates.io/api/v1/crates/$PACKAGE_NAME/versions")
PUBLISHED=$(echo "$CRATES_RESPONSE" | jq -r --arg ver "$VERSION" '.versions[] | select(.num == $ver) | .num // empty' 2>/dev/null || echo "")

if [ "$PUBLISHED" = "$VERSION" ]; then
    echo "⚠️  Version $VERSION is already published. Exiting."
    exit 0
else
    echo "✅ Version $VERSION is not yet published. Proceeding with publish."
fi

# Install cargo-workspaces
echo "📥 Installing cargo-workspaces..."
cargo install --git https://github.com/miraclx/cargo-workspaces --tag v0.3.0 cargo-workspaces

# Publish all crates
echo "🚀 Publishing all crates..."
set -x
cargo ws publish --yes --allow-dirty --force '*' \
    --no-git-commit --no-git-push --no-individual-tags --tag-prefix 'crates-' \
    --tag-msg $$'crates.io snapshot\n---%{\n- %n - https://crates.io/crates/%n/%v}' \
    --from-git

echo "✅ Publishing workflow completed!"
