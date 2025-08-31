#!/bin/bash
# Script to rebuild and push the local merod image
# Run this whenever you make changes to the core code
set -e
echo "🔨 Building merod image..."

# Create a temporary Dockerfile.build without the --locked flag
cp Dockerfile Dockerfile.build
sed -i '' 's/--locked//g' Dockerfile.build

docker build -f Dockerfile.build -t localhost:5001/merod:latest .

echo "📤 Pushing to local registry..."
docker push localhost:5001/merod:latest

echo "🧹 Cleaning up..."
rm Dockerfile.build

echo "✅ Image rebuilt and pushed successfully!"
echo "🚀 You can now run: merobox bootstrap run workflows/bootstrap-short.yml"
