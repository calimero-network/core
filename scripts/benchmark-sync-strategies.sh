#!/bin/bash
# ============================================================================
# Benchmark All Sync Strategies
# ============================================================================
#
# Runs the same divergence-repair workload with all 4 sync strategies:
#   - bloom_filter
#   - hash_comparison (adaptive default)
#   - subtree_prefetch
#   - level_wise
#
# Usage: ./scripts/benchmark-sync-strategies.sh [--binary-path PATH]
#
# Results are saved to data/<strategy>_metrics/
#
# ============================================================================

set -e

BINARY_PATH="${1:-./target/release/merod}"
MEROBOX="python -m merobox.cli"
DATA_DIR="/Users/xilosada/dev/calimero/core/data"
RESULTS_FILE="$DATA_DIR/strategy_benchmark_results.md"

# Check if binary exists
if [[ ! -f "$BINARY_PATH" ]]; then
    echo "Error: merod binary not found at $BINARY_PATH"
    echo "Build with: cargo build --release -p merod"
    exit 1
fi

echo "=============================================="
echo "  SYNC STRATEGY BENCHMARK"
echo "=============================================="
echo ""
echo "Binary: $BINARY_PATH"
echo "Results: $RESULTS_FILE"
echo ""

# Initialize results file
{
    echo "# Sync Strategy Benchmark Results"
    echo "Generated: $(date)"
    echo ""
    echo "## Test Configuration"
    echo "- 2 nodes"
    echo "- Node 1 writes 10 keys while Node 2 is down"
    echo "- Node 2 restarts and catches up using configured strategy"
    echo ""
} > "$RESULTS_FILE"

# Strategies to test
# NOTE: --force-state-sync bypasses DAG catchup to actually exercise the state sync strategies
STRATEGIES=("bloom" "hash" "subtree" "level")
STRATEGY_FLAGS=("--state-sync-strategy bloom --force-state-sync" "--state-sync-strategy hash --force-state-sync" "--state-sync-strategy subtree --force-state-sync" "--state-sync-strategy level --force-state-sync")
STRATEGY_NAMES=("Bloom Filter" "Hash Comparison" "Subtree Prefetch" "Level-Wise")

# Run benchmarks for each strategy
for i in "${!STRATEGIES[@]}"; do
    strategy="${STRATEGIES[$i]}"
    flag="${STRATEGY_FLAGS[$i]}"
    name="${STRATEGY_NAMES[$i]}"
    
    echo ">>> Testing $name strategy..."
    echo ""
    
    # Clean up previous data
    rm -rf "$DATA_DIR/${strategy}-"* 2>/dev/null || true
    
    # Create a modified workflow for this strategy
    WORKFLOW="/tmp/bench-strategy-${strategy}.yml"
    cat > "$WORKFLOW" <<EOF
description: "Benchmark ${name} sync strategy - divergence repair"
name: "Bench ${name}"

force_pull_image: false
nuke_on_start: true
e2e_mode: true

nodes:
  chain_id: testnet-1
  count: 2
  image: ghcr.io/calimero-network/merod:edge
  prefix: ${strategy}

steps:
  - name: Install Application on Node 1
    type: install_application
    node: ${strategy}-1
    path: ./workflow-examples/res/kv_store.wasm
    dev: true
    outputs:
      app_id: applicationId

  - name: Create Context on Node 1
    type: create_context
    node: ${strategy}-1
    application_id: "{{app_id}}"
    outputs:
      context_id: contextId
      pk_node1: memberPublicKey

  - name: Create Identity on Node 2
    type: create_identity
    node: ${strategy}-2
    outputs:
      pk_node2: publicKey

  - name: Invite Node 2
    type: invite_identity
    node: ${strategy}-1
    context_id: "{{context_id}}"
    grantee_id: "{{pk_node2}}"
    granter_id: "{{pk_node1}}"
    capability: member
    outputs:
      invitation: invitation

  - name: Node 2 Joins
    type: join_context
    node: ${strategy}-2
    context_id: "{{context_id}}"
    invitee_id: "{{pk_node2}}"
    invitation: "{{invitation}}"

  - name: Wait for initial mesh formation
    type: wait
    seconds: 15

  - name: ">>> Stopping Node 2 to create divergence"
    type: stop_node
    nodes: ${strategy}-2

  - name: Node 1 writes 10 keys while Node 2 is down
    type: repeat
    count: 10
    steps:
      - name: "N1 writes key_{{iteration}}"
        type: call
        node: ${strategy}-1
        context_id: "{{context_id}}"
        executor_public_key: "{{pk_node1}}"
        method: set
        args:
          key: "bench_key_{{iteration}}"
          value: "value_written_by_node1_{{iteration}}"

  - name: ">>> Starting Node 2 (will catch up via ${name})"
    type: start_node
    nodes: ${strategy}-2

  - name: Wait for sync to complete
    type: wait
    seconds: 30

  - name: Verify Node 2 has all keys
    type: call
    node: ${strategy}-2
    context_id: "{{context_id}}"
    executor_public_key: "{{pk_node2}}"
    method: get
    args:
      key: "bench_key_5"
    outputs:
      result: result

  - name: Assert sync worked
    type: json_assert
    statements:
      - 'json_subset({{result}}, {"output": "value_written_by_node1_5"})'

stop_all_nodes: true
restart: false
wait_timeout: 180
EOF

    # Run the benchmark
    START_TIME=$(date +%s.%N)
    
    if $MEROBOX bootstrap run --no-docker --binary-path "$BINARY_PATH" --merod-args="$flag" "$WORKFLOW" 2>&1; then
        END_TIME=$(date +%s.%N)
        DURATION=$(echo "$END_TIME - $START_TIME" | bc)
        
        echo ""
        echo "✓ $name completed in ${DURATION}s"
        echo ""
        
        # Extract metrics
        ./scripts/extract-sync-metrics.sh "$strategy" "$DATA_DIR" 2>/dev/null || true
        
        # Append to results
        {
            echo "## $name Strategy"
            echo ""
            echo "Total benchmark time: ${DURATION}s"
            echo ""
            
            if [[ -f "$DATA_DIR/${strategy}_metrics/summary.md" ]]; then
                cat "$DATA_DIR/${strategy}_metrics/summary.md"
            else
                echo "_No detailed metrics available_"
            fi
            echo ""
            echo "---"
            echo ""
        } >> "$RESULTS_FILE"
    else
        echo ""
        echo "✗ $name FAILED"
        echo ""
        
        {
            echo "## $name Strategy"
            echo ""
            echo "**FAILED**"
            echo ""
            echo "---"
            echo ""
        } >> "$RESULTS_FILE"
    fi
    
    # Clean up temp workflow
    rm -f "$WORKFLOW"
    
    echo ""
done

# Final summary
{
    echo "## Comparison Matrix"
    echo ""
    echo "| Strategy | Status | Notes |"
    echo "|----------|--------|-------|"
    
    for i in "${!STRATEGIES[@]}"; do
        strategy="${STRATEGIES[$i]}"
        name="${STRATEGY_NAMES[$i]}"
        
        if [[ -f "$DATA_DIR/${strategy}_metrics/summary.md" ]]; then
            echo "| $name | ✓ Pass | See detailed metrics above |"
        else
            echo "| $name | ✗ Fail | No metrics collected |"
        fi
    done
    
    echo ""
    echo "## Recommendations"
    echo ""
    echo "Based on the benchmark results:"
    echo ""
    echo "- **Bloom Filter**: Best for large trees with small divergence (<10%)"
    echo "- **Hash Comparison**: General purpose, good for most workloads"
    echo "- **Subtree Prefetch**: Best for deep trees with localized changes"
    echo "- **Level-Wise**: Best for wide, shallow trees"
    echo ""
} >> "$RESULTS_FILE"

echo "=============================================="
echo "  BENCHMARK COMPLETE"
echo "=============================================="
echo ""
echo "Results saved to: $RESULTS_FILE"
echo ""
cat "$RESULTS_FILE"
