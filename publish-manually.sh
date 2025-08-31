#!/bin/bash

# Script to manually publish all crates in dependency order
set -e

echo "🚀 Manual Publishing of All Calimero Crates"
echo "This will publish crates in dependency order to resolve version issues"

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

# Use the known workspace version
VERSION="0.2.1"
echo "📦 Using workspace version: $VERSION"

echo "📦 Publishing version: $VERSION"

# Define crates in dependency order (dependencies first, then dependent crates)
CRATES=(
    # Layer 1: Base crates with no internal dependencies
    "calimero-storage-macros"
    "calimero-storage"
)

echo "📋 Will publish ${#CRATES[@]} crates in this order:"
for i in "${!CRATES[@]}"; do
    echo "  $((i+1)). ${CRATES[$i]}"
done

echo ""
read -p "Continue with publishing? (y/N): " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "❌ Publishing cancelled"
    exit 0
fi

# Publish each crate
SKIPPED=0
PUBLISHED=0
FAILED=0

for crate in "${CRATES[@]}"; do
    echo ""
    echo "🚀 Publishing $crate..."
    
    # Check if already published
    echo "  🔍 Checking if $crate version $VERSION is already published..."
    if curl -s "https://crates.io/api/v1/crates/$crate/versions" | jq -r --arg ver "$VERSION" '.versions[] | select(.num == $ver) | .num // empty' 2>/dev/null | grep -q "$VERSION"; then
        echo "  ✅ $crate version $VERSION already published, skipping"
        ((SKIPPED++))
        continue
    else
        echo "  📦 $crate version $VERSION not found, proceeding with publish"
    fi
    
    # Publish the crate
    echo "  📤 Publishing $crate version $VERSION..."
    if cargo publish --package "$crate" --allow-dirty; then
        echo "  ✅ Successfully published $crate"
        ((PUBLISHED++))
    else
        echo "  ❌ Failed to publish $crate"
        ((FAILED++))
        echo "  💡 You may need to fix dependency issues before continuing"
        read -p "Continue with remaining crates? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            echo "❌ Publishing stopped due to failure"
            break
        fi
    fi
    
    # Small delay to avoid overwhelming crates.io
    sleep 2
done

echo ""
echo "📊 Publishing Summary:"
echo "  ✅ Skipped (already published): $SKIPPED crates"
echo "  📦 Successfully published: $PUBLISHED crates"
echo "  ❌ Failed to publish: $FAILED crates"
echo "  📋 Total processed: $((SKIPPED + PUBLISHED + FAILED)) crates"
echo ""
echo "🎉 Manual publishing completed!"
echo "Now you can use the regular publish-locally.sh script for future updates"
