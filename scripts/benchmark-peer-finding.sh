#!/bin/bash
# ============================================================================
# Benchmark Peer Finding Strategies
# ============================================================================
#
# Runs all peer finding strategies (A0-A5) across multiple scenarios:
# - Warm steady-state (10 nodes, stable)
# - Cold start (fresh context join)
# - Churn restart (node restarts during writes)
# - Partition heal (5/5 split)
# - Dial storm (10 nodes start simultaneously)
#
# Usage: ./scripts/benchmark-peer-finding.sh [binary_path]
#
# ============================================================================

set -e

BINARY="${1:-./target/release/merod}"
DATA_DIR="./data"
RESULTS_FILE="$DATA_DIR/peer_find_benchmark_results.md"

# Strategies to test
STRATEGIES=(
    "baseline"      # A0: Current mesh-only
    "recent-first"  # A2: LRU cache first, then mesh
    "health-filtered" # A5: Exclude failed peers
)

# Scenarios (workflow files)
declare -A SCENARIOS=(
    ["warm"]="workflows/sync/edge-cold-dial-storm.yml"
    ["churn"]="workflows/sync/edge-churn-reconnect.yml"
    ["partition"]="workflows/sync/edge-partition-healing.yml"
)

echo "=============================================="
echo "  PEER FINDING BENCHMARK"
echo "=============================================="
echo ""
echo "Binary: $BINARY"
echo "Strategies: ${STRATEGIES[*]}"
echo "Scenarios: ${!SCENARIOS[*]}"
echo ""

# Ensure binary exists
if [[ ! -x "$BINARY" ]]; then
    echo "Error: Binary not found or not executable: $BINARY"
    echo "Run: cargo build --release -p merod"
    exit 1
fi

# Initialize results file
{
    echo "# Peer Finding Benchmark Results"
    echo ""
    echo "**Date**: $(date)"
    echo "**Binary**: $BINARY"
    echo ""
    echo "## Results Summary"
    echo ""
    echo "| Strategy | Scenario | peer_find_total P50 | P95 | P99 | Success Rate |"
    echo "|----------|----------|---------------------|-----|-----|--------------|"
} > "$RESULTS_FILE"

# Function to run a single benchmark
run_benchmark() {
    local strategy="$1"
    local scenario_name="$2"
    local workflow="$3"
    local prefix="${scenario_name}-${strategy}"
    
    echo ">>> Running: $scenario_name with $strategy strategy..."
    
    # Clean previous data
    rm -rf "$DATA_DIR"/${prefix}-* 2>/dev/null || true
    
    # Run the workflow with the specified strategy
    if python -m merobox.cli bootstrap run \
        --no-docker \
        --binary-path "$BINARY" \
        --merod-args="--peer-find-strategy $strategy" \
        "$workflow" 2>&1 | tail -5; then
        
        # Extract peer finding metrics
        local metrics=$(./scripts/extract-sync-metrics.sh "$prefix" 2>/dev/null | grep -A10 "PEER FINDING")
        
        # Parse P50, P95, P99 from metrics
        local p50=$(echo "$metrics" | grep "P50:" | head -1 | grep -oE 'P50: [0-9.]+' | cut -d' ' -f2)
        local p95=$(echo "$metrics" | grep "P95:" | head -1 | grep -oE 'P95: [0-9.]+' | cut -d' ' -f2)
        local p99=$(echo "$metrics" | grep "P99:" | head -1 | grep -oE 'P99: [0-9.]+' | cut -d' ' -f2)
        
        [[ -z "$p50" ]] && p50="N/A"
        [[ -z "$p95" ]] && p95="N/A"
        [[ -z "$p99" ]] && p99="N/A"
        
        echo "| $strategy | $scenario_name | ${p50}ms | ${p95}ms | ${p99}ms | ✅ |" >> "$RESULTS_FILE"
        echo "    ✓ Completed: P50=${p50}ms P95=${p95}ms P99=${p99}ms"
    else
        echo "| $strategy | $scenario_name | N/A | N/A | N/A | ❌ |" >> "$RESULTS_FILE"
        echo "    ✗ Failed"
    fi
    echo ""
}

# Run all combinations
for scenario_name in "${!SCENARIOS[@]}"; do
    workflow="${SCENARIOS[$scenario_name]}"
    echo ""
    echo "============ SCENARIO: $scenario_name ============"
    
    for strategy in "${STRATEGIES[@]}"; do
        run_benchmark "$strategy" "$scenario_name" "$workflow"
    done
done

# Add summary
{
    echo ""
    echo "## Analysis"
    echo ""
    echo "### Key Findings"
    echo ""
    echo "1. **Baseline (A0)**: [TBD after results]"
    echo "2. **Recent-First (A2)**: [TBD after results]"
    echo "3. **Health-Filtered (A5)**: [TBD after results]"
    echo ""
    echo "### Recommendation"
    echo ""
    echo "[TBD after results]"
    echo ""
} >> "$RESULTS_FILE"

echo "=============================================="
echo "  BENCHMARK COMPLETE"
echo "=============================================="
echo ""
echo "Results saved to: $RESULTS_FILE"
cat "$RESULTS_FILE"
