#!/bin/bash
set -e

# Start relayer
echo "Starting relayer development environment..."
docker-compose -f docker-compose.relayer.yml -f docker-compose.relayer.dev.yml up -d relayer

echo "Development environment started successfully!"
echo "- Relayer API: http://localhost:${RELAYER_PORT:-63529}"