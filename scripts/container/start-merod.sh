#!/bin/sh
set -e

# Environment variables with defaults
CALIMERO_HOME="${CALIMERO_HOME:-/data}"
CALIMERO_NODE="${NODE_NAME:-default}"
SERVER_PORT="2428"
SWARM_PORT="2528"

echo "Initializing merod..."
merod --home "$CALIMERO_HOME" \
    --node-name "$CALIMERO_NODE" \
    init \
    --advertise-address \
    --server-host 0.0.0.0 \
    --server-port "$SERVER_PORT" \
    --swarm-port "$SWARM_PORT"

echo "Starting merod..."
exec merod --home "$CALIMERO_HOME" \
    --node-name "$CALIMERO_NODE" \
    run
