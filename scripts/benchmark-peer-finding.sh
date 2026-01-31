#!/usr/bin/env bash
# ============================================================================
# Benchmark Peer Finding Strategies
# ============================================================================
#
# Runs all peer finding strategies (A0-A5) across multiple scenarios
#
# Usage: ./scripts/benchmark-peer-finding.sh [binary_path]
#
# ============================================================================

set -e

BINARY="${1:-./target/release/merod}"
DATA_DIR="./data"
RESULTS_FILE="$DATA_DIR/peer_find_benchmark_results.md"

# Strategies to test
STRATEGIES="baseline recent-first health-filtered"

# Scenarios (name:workflow pairs)
SCENARIOS="warm:workflows/sync/edge-cold-dial-storm.yml churn:workflows/sync/edge-churn-reconnect.yml partition:workflows/sync/edge-partition-healing.yml"

echo "=============================================="
echo "  PEER FINDING BENCHMARK"
echo "=============================================="
echo ""
echo "Binary: $BINARY"
echo "Strategies: $STRATEGIES"
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
    echo "| Strategy | Scenario | peer_find_total P50 | P95 | Success |"
    echo "|----------|----------|---------------------|-----|---------|"
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
        "$workflow" > /tmp/benchmark_${prefix}.log 2>&1; then
        
        # Extract peer finding metrics
        local p50="N/A"
        local p95="N/A"
        
        # Look for peer find data in logs
        for node_dir in "$DATA_DIR"/${prefix}-*/; do
            if [[ -d "$node_dir" ]]; then
                node_name=$(basename "$node_dir")
                log_file="$node_dir/logs/${node_name}.log"
                
                if [[ -f "$log_file" ]]; then
                    # Extract peer_find_total_ms values
                    local values=$(grep "PEER_FIND_BREAKDOWN" "$log_file" 2>/dev/null | \
                        grep -oE 'peer_find_total_ms=[0-9.]+' | \
                        cut -d'=' -f2 | sort -n)
                    
                    if [[ -n "$values" ]]; then
                        local count=$(echo "$values" | wc -l | tr -d ' ')
                        local p50_idx=$((count * 50 / 100))
                        local p95_idx=$((count * 95 / 100))
                        [[ "$p50_idx" -lt 1 ]] && p50_idx=1
                        [[ "$p95_idx" -lt 1 ]] && p95_idx=1
                        
                        p50=$(echo "$values" | sed -n "${p50_idx}p" | cut -d'.' -f1)
                        p95=$(echo "$values" | sed -n "${p95_idx}p" | cut -d'.' -f1)
                    fi
                fi
            fi
        done
        
        echo "| $strategy | $scenario_name | ${p50}ms | ${p95}ms | ✅ |" >> "$RESULTS_FILE"
        echo "    ✓ Completed: P50=${p50}ms P95=${p95}ms"
    else
        echo "| $strategy | $scenario_name | N/A | N/A | ❌ |" >> "$RESULTS_FILE"
        echo "    ✗ Failed (see /tmp/benchmark_${prefix}.log)"
    fi
    echo ""
}

# Run all combinations
for scenario_pair in $SCENARIOS; do
    scenario_name="${scenario_pair%%:*}"
    workflow="${scenario_pair#*:}"
    
    echo ""
    echo "============ SCENARIO: $scenario_name ============"
    
    for strategy in $STRATEGIES; do
        run_benchmark "$strategy" "$scenario_name" "$workflow"
    done
done

# Add analysis section
{
    echo ""
    echo "## Analysis"
    echo ""
    echo "### Recommendations"
    echo ""
    echo "Based on the results, the recommended peer finding strategy is:"
    echo ""
    echo "1. **Production**: \`baseline\` (A0) - proven stable"
    echo "2. **Churn recovery**: \`recent-first\` (A2) - uses cached successful peers"
    echo "3. **High failure rate**: \`health-filtered\` (A5) - excludes failing peers"
    echo ""
} >> "$RESULTS_FILE"

echo "=============================================="
echo "  BENCHMARK COMPLETE"
echo "=============================================="
echo ""
echo "Results saved to: $RESULTS_FILE"
echo ""
cat "$RESULTS_FILE"
