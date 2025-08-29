#!/bin/bash

# Build script for Calimero Client Python Bindings
# Run this from the python/ directory

set -e

echo "🔧 Building Calimero Client Python Bindings..."

# Check if we're in the right directory
if [ ! -f "pyproject.toml" ]; then
    echo "❌ Error: pyproject.toml not found. Please run this script from the python/ directory."
    exit 1
fi

# Check if maturin is installed
if ! command -v maturin &> /dev/null; then
    echo "📦 Installing maturin..."
    pip install maturin
fi

# Clean previous builds
echo "🧹 Cleaning previous builds..."
rm -rf target/wheels/
rm -rf dist/
rm -rf build/
rm -rf *.egg-info/

# Build the package from the parent directory (where Cargo.toml is)
echo "🏗️ Building with maturin from parent directory..."
cd .. && maturin build --features python --release --manifest-path Cargo.toml

# Move back to python directory
cd python

# Show the result
echo "✅ Build complete!"
echo "📦 Wheel created at:"
ls -la ../target/wheels/

echo ""
echo "🚀 To install the package:"
echo "pip install ../../target/wheels/calimero_client_py_bindings-*.whl"
echo ""
echo "🧪 To test the package:"
echo "python -c \"import calimero_client_py_bindings; print('Import successful')\""
