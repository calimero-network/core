#!/bin/bash

# Setup Husky for Rust project
# This script installs and configures Husky for pre-commit hooks

set -e

echo "🔧 Setting up Husky for Rust project..."

# Check if npm is available
if ! command -v npm &> /dev/null; then
    echo "❌ npm is not installed. Please install Node.js and npm first."
    echo "   Visit: https://nodejs.org/"
    exit 1
fi

# Check if git is available
if ! command -v git &> /dev/null; then
    echo "❌ git is not installed. Please install git first."
    exit 1
fi

# Initialize npm if package.json doesn't exist
if [ ! -f "package.json" ]; then
    echo "📦 Initializing npm package.json..."
    npm init -y
fi

# Install husky as dev dependency
echo "📦 Installing Husky..."
npm install --save-dev husky

# Enable git hooks
echo "🔗 Enabling git hooks..."
npx husky install

# Make sure the pre-commit hook is executable
chmod +x .husky/pre-commit

echo "✅ Husky setup complete!"
echo ""
echo "🎯 Pre-commit hooks will now run:"
echo "   - Rust code formatting (cargo fmt)"
echo "   - Rust linting (cargo clippy)"
echo "   - TOML syntax validation"
echo "   - YAML syntax validation"
echo ""
echo "💡 To test the setup, try making a change and committing:"
echo "   git add . && git commit -m 'test: testing pre-commit hooks'"
