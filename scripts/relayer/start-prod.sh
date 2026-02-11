#!/bin/bash
set -e

# Start relayer
echo "Starting relayer..."
docker-compose -f docker-compose.relayer.yml up -d relayer

echo "Relayer started successfully!"
echo "- Relayer API: http://localhost:${RELAYER_PORT:-63529}"