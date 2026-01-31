#!/bin/bash
# Manual Benchmark: Compare sync strategies
#
# Usage: ./scripts/manual-benchmark.sh [snapshot|delta]

set -e

STRATEGY="${1:-snapshot}"
MEROD="./target/release/merod"
DATA_DIR="data/bench-$STRATEGY"
NODE_NAME="bench-$STRATEGY-node"

echo "=============================================="
echo "  Manual Benchmark: $STRATEGY strategy"
echo "=============================================="

# Clean previous data
rm -rf "$DATA_DIR"

# Initialize node
echo "Initializing node..."
$MEROD --node-name "$NODE_NAME" --home "$DATA_DIR" init --server-port 2530

# Start node with strategy
echo ""
echo "Starting node with --sync-strategy $STRATEGY"
echo "Watch for sync messages..."
echo ""

RUST_LOG=info $MEROD \
    --node-name "$NODE_NAME" \
    --home "$DATA_DIR" \
    run \
    --sync-strategy "$STRATEGY" \
    --state-sync-strategy adaptive
