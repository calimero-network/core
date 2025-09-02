#!/bin/bash

echo "🧹 Cleaning up containers and data..."

# Stop and remove all containers
echo "📦 Stopping and removing containers..."
docker stop $(docker ps -q) 2>/dev/null || true
docker rm $(docker ps -aq) 2>/dev/null || true

# Remove data directory
echo "🗑️  Removing data directory..."
rm -rf data/

echo "🔨 Rebuilding Docker image with latest storage fixes..."
docker build -t merod-sdk-test-debug-paths:latest .

echo "✨ Cleanup and rebuild complete! Now running merobox..."
echo ""

# Run the workflow
merobox bootstrap run workflows/collection-storage-test.yml
