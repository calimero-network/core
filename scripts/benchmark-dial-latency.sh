#!/bin/bash
# ============================================================================
# Benchmark Dial Latency (Phase 2 Optimization)
# ============================================================================
#
# Runs dial latency benchmarks to measure connection establishment time.
# Extracts PEER_DIAL_BREAKDOWN metrics to analyze:
#   - Warm vs cold connection dial time
#   - Connection reuse rate
#   - Dial success/failure distribution
#
# Usage: ./scripts/benchmark-dial-latency.sh
#
# ============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DATA_DIR="$PROJECT_ROOT/data"

echo "=============================================="
echo "Phase 2: Dial Latency Benchmark Suite"
echo "=============================================="
echo ""
echo "Date: $(date)"
echo "Branch: $(git branch --show-current)"
echo ""

# Check if merobox is available
if ! command -v merobox &> /dev/null; then
    echo "ERROR: merobox not found. Please install merobox first."
    exit 1
fi

# ============================================================================
# Benchmark 1: Warm Connection Dial
# ============================================================================

echo "=============================================="
echo "Benchmark 1: Warm Connection Dial Latency"
echo "=============================================="
echo ""
echo "Testing dial latency with established connections (back-to-back syncs)"
echo ""

# Clean up any existing data
rm -rf "$DATA_DIR"/dial-*

merobox run "$PROJECT_ROOT/workflows/sync/bench-dial-warm.yml" \
    --data-dir "$DATA_DIR" \
    || echo "Warm dial benchmark completed (check logs for results)"

echo ""
echo "Extracting warm dial metrics..."
"$SCRIPT_DIR/extract-sync-metrics.sh" "dial" "$DATA_DIR" 2>/dev/null || true

# Save results
WARM_RESULTS_DIR="$DATA_DIR/dial_warm_results"
mkdir -p "$WARM_RESULTS_DIR"
cp -r "$DATA_DIR"/dial-* "$WARM_RESULTS_DIR/" 2>/dev/null || true
cp -r "$DATA_DIR/dial_metrics" "$WARM_RESULTS_DIR/" 2>/dev/null || true

echo ""

# ============================================================================
# Benchmark 2: Cold Connection Dial
# ============================================================================

echo "=============================================="
echo "Benchmark 2: Cold Connection Dial Latency"
echo "=============================================="
echo ""
echo "Testing dial latency after node restart (new connections)"
echo ""

# Clean up
rm -rf "$DATA_DIR"/dial-*

merobox run "$PROJECT_ROOT/workflows/sync/bench-dial-cold.yml" \
    --data-dir "$DATA_DIR" \
    || echo "Cold dial benchmark completed (check logs for results)"

echo ""
echo "Extracting cold dial metrics..."
"$SCRIPT_DIR/extract-sync-metrics.sh" "dial" "$DATA_DIR" 2>/dev/null || true

# Save results
COLD_RESULTS_DIR="$DATA_DIR/dial_cold_results"
mkdir -p "$COLD_RESULTS_DIR"
cp -r "$DATA_DIR"/dial-* "$COLD_RESULTS_DIR/" 2>/dev/null || true
cp -r "$DATA_DIR/dial_metrics" "$COLD_RESULTS_DIR/" 2>/dev/null || true

echo ""

# ============================================================================
# Summary
# ============================================================================

echo "=============================================="
echo "Dial Latency Benchmark Summary"
echo "=============================================="
echo ""

# Extract key metrics from results
echo "=== Warm Connection Dial (back-to-back syncs) ==="
if [[ -f "$WARM_RESULTS_DIR/dial_metrics/dial_breakdown_raw.csv" ]]; then
    WARM_COUNT=$(wc -l < "$WARM_RESULTS_DIR/dial_metrics/dial_breakdown_raw.csv" | tr -d ' ')
    WARM_AVG=$(cut -d',' -f1 "$WARM_RESULTS_DIR/dial_metrics/dial_breakdown_raw.csv" | awk '{sum+=$1;count++} END {if(count>0) printf "%.2f", sum/count; else print "N/A"}')
    echo "  Dial attempts: $WARM_COUNT"
    echo "  Avg dial time: ${WARM_AVG}ms"
else
    echo "  No warm dial data found"
fi

echo ""
echo "=== Cold Connection Dial (after restart) ==="
if [[ -f "$COLD_RESULTS_DIR/dial_metrics/dial_breakdown_raw.csv" ]]; then
    COLD_COUNT=$(wc -l < "$COLD_RESULTS_DIR/dial_metrics/dial_breakdown_raw.csv" | tr -d ' ')
    COLD_AVG=$(cut -d',' -f1 "$COLD_RESULTS_DIR/dial_metrics/dial_breakdown_raw.csv" | awk '{sum+=$1;count++} END {if(count>0) printf "%.2f", sum/count; else print "N/A"}')
    echo "  Dial attempts: $COLD_COUNT"
    echo "  Avg dial time: ${COLD_AVG}ms"
else
    echo "  No cold dial data found"
fi

echo ""
echo "=============================================="
echo "Full results saved to:"
echo "  Warm: $WARM_RESULTS_DIR"
echo "  Cold: $COLD_RESULTS_DIR"
echo "=============================================="
