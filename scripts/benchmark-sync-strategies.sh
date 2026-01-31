#!/bin/bash
# Benchmark Fresh Node Sync Strategies
#
# Usage: ./scripts/benchmark-sync-strategies.sh [strategy]
#   strategy: snapshot (default), delta, adaptive
#
# Example:
#   ./scripts/benchmark-sync-strategies.sh snapshot
#   ./scripts/benchmark-sync-strategies.sh delta

set -e

STRATEGY="${1:-snapshot}"
BINARY_PATH="./target/release/merod"
WORKFLOW="workflows/sync/benchmark-fresh-node-strategies.yml"
LOG_FILE="/tmp/benchmark_${STRATEGY}_$(date +%s).log"

echo "=============================================="
echo "  Benchmark: Fresh Node Sync Strategy"
echo "=============================================="
echo "Strategy: $STRATEGY"
echo "Binary:   $BINARY_PATH"
echo "Log:      $LOG_FILE"
echo "=============================================="

# Clean up old data
rm -rf data/benchmark-*

# Check if merobox is available
if ! command -v merobox &> /dev/null; then
    echo "ERROR: merobox not found. Install with: pipx install merobox"
    exit 1
fi

# Run benchmark with timing
echo ""
echo "Starting benchmark at $(date)"
START_TIME=$(date +%s.%N)

# Run the benchmark - override sync strategy via env or modify workflow
# For now, we need to modify the merod call in merobox
# This is a placeholder - actual implementation depends on merobox supporting args

RUST_LOG=info merobox bootstrap run \
    --no-docker \
    --binary-path "$BINARY_PATH" \
    "$WORKFLOW" 2>&1 | tee "$LOG_FILE"

END_TIME=$(date +%s.%N)
DURATION=$(echo "$END_TIME - $START_TIME" | bc)

echo ""
echo "=============================================="
echo "  Results"
echo "=============================================="
echo "Strategy:  $STRATEGY"
echo "Duration:  ${DURATION}s"
echo ""

# Extract key metrics from logs
echo "Key metrics from logs:"
grep -E "(Snapshot sync completed|Delta pending|request_delta|Using fresh node sync strategy|Applied.*records)" "$LOG_FILE" | head -20 || true

echo ""
echo "Full log: $LOG_FILE"
echo "=============================================="
