#!/bin/bash

# Build script for mero-py
# This script builds the Python package using maturin

set -e

echo "Building mero-py..."

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "❌ Error: Cargo.toml not found. Please run this script from the project root directory."
    exit 1
fi

# Check if maturin is installed
if ! command -v maturin &> /dev/null; then
    echo "❌ Maturin not found!"
    echo ""
    echo "📦 To install maturin, you have several options:"
    echo ""
    echo "Option 1: Use a virtual environment (recommended)"
    echo "  python3 -m venv venv"
    echo "  source venv/bin/activate"
    echo "  pip install maturin"
    echo ""
    echo "Option 2: Use pipx (if available)"
    echo "  brew install pipx"
    echo "  pipx install maturin"
    echo ""
    echo "Option 3: Install globally (not recommended)"
    echo "  python3 -m pip install --user maturin"
    echo ""
    echo "After installing maturin, run this script again."
    exit 1
fi

# Clean previous builds
echo "🧹 Cleaning previous builds..."
rm -rf target/ dist/ build/ *.egg-info/

# Build the bindings
echo "🔨 Building with maturin..."
maturin build --release

echo "✅ Build completed successfully!"
echo "📦 Wheel files created in dist/ directory"

# Optional: install in development mode
if [ "$1" = "--install" ]; then
    echo "📥 Installing in development mode..."
    maturin develop --release
    echo "✅ Installed successfully!"
fi
