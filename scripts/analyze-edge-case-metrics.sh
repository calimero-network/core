#!/bin/bash
# ============================================================================
# Analyze Edge Case Benchmark Metrics
# ============================================================================
#
# Usage: ./scripts/analyze-edge-case-metrics.sh <data_prefix>
# Example: ./scripts/analyze-edge-case-metrics.sh dial
#
# Extracts and analyzes:
#   - peer_selection P50/P95/P99
#   - total_sync P50/P95/P99
#   - sync success/failure rates
#   - STRATEGY_SYNC_METRICS
#   - Tail latency breakdown (which phase dominates slow syncs)
#
# ============================================================================

set -e

PREFIX="${1:-dial}"
DATA_DIR="${2:-/Users/xilosada/dev/calimero/core/data}"
OUTPUT_DIR="$DATA_DIR/${PREFIX}_analysis"

mkdir -p "$OUTPUT_DIR"

echo "=============================================="
echo "  EDGE CASE ANALYSIS: $PREFIX"
echo "=============================================="
echo ""

# ============================================================================
# Function: Calculate percentiles
# ============================================================================
calc_percentiles() {
    local file="$1"
    local name="$2"
    
    if [[ ! -s "$file" ]]; then
        echo "$name: No data"
        return
    fi
    
    local sorted=$(sort -n "$file" 2>/dev/null | grep -v '^$')
    local count=$(echo "$sorted" | grep -c . 2>/dev/null || echo "0")
    
    if [[ "$count" -gt 0 ]]; then
        local min=$(echo "$sorted" | head -1)
        local max=$(echo "$sorted" | tail -1)
        local sum=$(echo "$sorted" | awk '{sum+=$1} END {print sum}')
        local avg=$(echo "scale=2; $sum / $count" | bc 2>/dev/null || echo "0")
        
        local p50_idx=$(echo "($count * 50 + 50) / 100" | bc)
        local p95_idx=$(echo "($count * 95 + 50) / 100" | bc)
        local p99_idx=$(echo "($count * 99 + 50) / 100" | bc)
        [[ "$p50_idx" -lt 1 ]] && p50_idx=1
        [[ "$p95_idx" -lt 1 ]] && p95_idx=1
        [[ "$p99_idx" -lt 1 ]] && p99_idx=1
        [[ "$p50_idx" -gt "$count" ]] && p50_idx="$count"
        [[ "$p95_idx" -gt "$count" ]] && p95_idx="$count"
        [[ "$p99_idx" -gt "$count" ]] && p99_idx="$count"
        
        local p50=$(echo "$sorted" | sed -n "${p50_idx}p")
        local p95=$(echo "$sorted" | sed -n "${p95_idx}p")
        local p99=$(echo "$sorted" | sed -n "${p99_idx}p")
        
        echo "$name (n=$count):"
        echo "  Min: ${min}ms  Max: ${max}ms  Avg: ${avg}ms"
        echo "  P50: ${p50}ms  P95: ${p95}ms  P99: ${p99}ms"
        
        # Save to CSV
        echo "$name,$count,$min,$max,$avg,$p50,$p95,$p99" >> "$OUTPUT_DIR/metrics.csv"
    fi
}

# Initialize CSV
echo "metric,count,min,max,avg,p50,p95,p99" > "$OUTPUT_DIR/metrics.csv"

# ============================================================================
# Extract SYNC_PHASE_BREAKDOWN metrics
# ============================================================================
echo ">>> Extracting SYNC_PHASE_BREAKDOWN..."
echo ""

PEER_SEL_FILE=$(mktemp)
KEY_SHARE_FILE=$(mktemp)
DAG_COMPARE_FILE=$(mktemp)
DATA_XFER_FILE=$(mktemp)
TOTAL_SYNC_FILE=$(mktemp)

for node_dir in "$DATA_DIR"/${PREFIX}-*/; do
    if [[ -d "$node_dir" ]]; then
        node_name=$(basename "$node_dir")
        log_file="$node_dir/logs/${node_name}.log"
        
        if [[ -f "$log_file" ]]; then
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                grep -oE 'peer_selection_ms="[0-9.]+"' | \
                sed 's/peer_selection_ms="//;s/"//' >> "$PEER_SEL_FILE" 2>/dev/null || true
            
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                grep -oE 'key_share_ms="[0-9.]+"' | \
                sed 's/key_share_ms="//;s/"//' >> "$KEY_SHARE_FILE" 2>/dev/null || true
            
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                grep -oE 'dag_compare_ms="[0-9.]+"' | \
                sed 's/dag_compare_ms="//;s/"//' >> "$DAG_COMPARE_FILE" 2>/dev/null || true
            
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                grep -oE 'data_transfer_ms="[0-9.]+"' | \
                sed 's/data_transfer_ms="//;s/"//' >> "$DATA_XFER_FILE" 2>/dev/null || true
            
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                grep -oE 'total_ms="[0-9.]+"' | \
                sed 's/total_ms="//;s/"//' >> "$TOTAL_SYNC_FILE" 2>/dev/null || true
        fi
    fi
done

echo "=== SYNC PHASE TIMING ==="
echo ""
calc_percentiles "$PEER_SEL_FILE" "peer_selection_ms"
calc_percentiles "$KEY_SHARE_FILE" "key_share_ms"
calc_percentiles "$DAG_COMPARE_FILE" "dag_compare_ms"
calc_percentiles "$DATA_XFER_FILE" "data_transfer_ms"
calc_percentiles "$TOTAL_SYNC_FILE" "total_sync_ms"
echo ""

rm -f "$PEER_SEL_FILE" "$KEY_SHARE_FILE" "$DAG_COMPARE_FILE" "$DATA_XFER_FILE" "$TOTAL_SYNC_FILE"

# ============================================================================
# Extract STRATEGY_SYNC_METRICS
# ============================================================================
echo ">>> Extracting STRATEGY_SYNC_METRICS..."
echo ""

STRATEGY_FILE=$(mktemp)
for node_dir in "$DATA_DIR"/${PREFIX}-*/; do
    if [[ -d "$node_dir" ]]; then
        node_name=$(basename "$node_dir")
        log_file="$node_dir/logs/${node_name}.log"
        
        if [[ -f "$log_file" ]]; then
            grep "STRATEGY_SYNC_METRICS" "$log_file" 2>/dev/null >> "$STRATEGY_FILE" || true
        fi
    fi
done

if [[ -s "$STRATEGY_FILE" ]]; then
    echo "=== STRATEGY SYNC METRICS ==="
    echo ""
    
    for strategy in bloom_filter hash_comparison subtree_prefetch level_wise; do
        STRAT_DURATION=$(mktemp)
        STRAT_TRIPS=$(mktemp)
        
        grep "strategy=\"$strategy\"" "$STRATEGY_FILE" | \
            grep -oE 'duration_ms="[0-9.]+"' | \
            sed 's/duration_ms="//;s/"//' > "$STRAT_DURATION" 2>/dev/null || true
        
        grep "strategy=\"$strategy\"" "$STRATEGY_FILE" | \
            grep -oE 'round_trips=[0-9]+' | \
            sed 's/round_trips=//' > "$STRAT_TRIPS" 2>/dev/null || true
        
        count=$(wc -l < "$STRAT_DURATION" 2>/dev/null | tr -d ' ')
        [[ -z "$count" || ! "$count" =~ ^[0-9]+$ ]] && count=0
        
        if [[ "$count" -gt 0 ]]; then
            echo "--- $strategy ---"
            calc_percentiles "$STRAT_DURATION" "  duration"
            avg_trips=$(awk '{sum+=$1} END {if(NR>0) printf "%.1f", sum/NR; else print "N/A"}' "$STRAT_TRIPS")
            echo "  Avg round trips: $avg_trips"
            echo ""
        fi
        
        rm -f "$STRAT_DURATION" "$STRAT_TRIPS"
    done
fi
rm -f "$STRATEGY_FILE"

# ============================================================================
# Sync Success/Failure Analysis
# ============================================================================
echo ">>> Analyzing Sync Success/Failure..."
echo ""

TOTAL_ATTEMPTS=0
TOTAL_SUCCESS=0
TOTAL_FAILURES=0

for node_dir in "$DATA_DIR"/${PREFIX}-*/; do
    if [[ -d "$node_dir" ]]; then
        node_name=$(basename "$node_dir")
        log_file="$node_dir/logs/${node_name}.log"
        
        if [[ -f "$log_file" ]]; then
            attempts=$(grep -c "Starting sync interval" "$log_file" 2>/dev/null || echo "0")
            success=$(grep -c "Sync finished successfully" "$log_file" 2>/dev/null || echo "0")
            failures=$(grep -c "Sync failed" "$log_file" 2>/dev/null || echo "0")
            
            TOTAL_ATTEMPTS=$((TOTAL_ATTEMPTS + attempts))
            TOTAL_SUCCESS=$((TOTAL_SUCCESS + success))
            TOTAL_FAILURES=$((TOTAL_FAILURES + failures))
        fi
    fi
done

echo "=== SYNC SUCCESS/FAILURE ==="
echo ""
echo "Total sync attempts: $TOTAL_ATTEMPTS"
echo "Total successes: $TOTAL_SUCCESS"
echo "Total failures: $TOTAL_FAILURES"

if [[ "$TOTAL_ATTEMPTS" -gt 0 ]]; then
    SUCCESS_RATE=$(echo "scale=1; $TOTAL_SUCCESS * 100 / $TOTAL_ATTEMPTS" | bc 2>/dev/null || echo "N/A")
    echo "Success rate: ${SUCCESS_RATE}%"
fi
echo ""

# ============================================================================
# Tail Latency Analysis (P95+ breakdown)
# ============================================================================
echo ">>> Analyzing Tail Latency (slow syncs)..."
echo ""

echo "=== TAIL LATENCY BREAKDOWN ==="
echo ""

# Find syncs where total_ms > P95
P95_THRESHOLD=500  # Default, will be computed

SLOW_SYNCS=$(mktemp)
for node_dir in "$DATA_DIR"/${PREFIX}-*/; do
    if [[ -d "$node_dir" ]]; then
        node_name=$(basename "$node_dir")
        log_file="$node_dir/logs/${node_name}.log"
        
        if [[ -f "$log_file" ]]; then
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                awk -F'total_ms="' '{print $2}' | \
                awk -F'"' '{if ($1+0 > 500) print $0}' >> "$SLOW_SYNCS" 2>/dev/null || true
        fi
    fi
done

SLOW_COUNT=$(wc -l < "$SLOW_SYNCS" 2>/dev/null | tr -d ' ')
echo "Syncs > 500ms: $SLOW_COUNT"

if [[ "$SLOW_COUNT" -gt 0 ]]; then
    echo ""
    echo "Sample slow syncs (first 5):"
    head -5 "$SLOW_SYNCS" | while read -r line; do
        peer_sel=$(echo "$line" | grep -oE 'peer_selection_ms="[0-9.]+"' | sed 's/peer_selection_ms="//;s/"//')
        total=$(echo "$line" | grep -oE 'total_ms="[0-9.]+"' | sed 's/total_ms="//;s/"//')
        echo "  total=${total}ms  peer_selection=${peer_sel}ms"
    done
fi
rm -f "$SLOW_SYNCS"
echo ""

# ============================================================================
# Mesh Formation Analysis
# ============================================================================
echo ">>> Analyzing Mesh Formation..."
echo ""

echo "=== MESH FORMATION ==="
echo ""

for node_dir in "$DATA_DIR"/${PREFIX}-*/; do
    if [[ -d "$node_dir" ]]; then
        node_name=$(basename "$node_dir")
        log_file="$node_dir/logs/${node_name}.log"
        
        if [[ -f "$log_file" ]]; then
            mesh_peers=$(grep -c "peers in mesh" "$log_file" 2>/dev/null || echo "0")
            mesh_empty=$(grep -c "mesh is empty" "$log_file" 2>/dev/null || echo "0")
            resubscribe=$(grep -c "Re-subscribing to topic" "$log_file" 2>/dev/null || echo "0")
            
            echo "$node_name: mesh_checks=$mesh_peers  empty_mesh=$mesh_empty  resubscribes=$resubscribe"
        fi
    fi
done
echo ""

# ============================================================================
# Generate Summary
# ============================================================================
echo "=============================================="
echo "  SUMMARY"
echo "=============================================="
echo ""

{
    echo "# Edge Case Analysis: $PREFIX"
    echo "Generated: $(date)"
    echo ""
    echo "## Key Metrics"
    echo ""
    cat "$OUTPUT_DIR/metrics.csv" | column -t -s','
    echo ""
    echo "## Sync Stats"
    echo ""
    echo "- Total attempts: $TOTAL_ATTEMPTS"
    echo "- Success rate: ${SUCCESS_RATE:-N/A}%"
    echo "- Slow syncs (>500ms): ${SLOW_COUNT:-0}"
    echo ""
} > "$OUTPUT_DIR/summary.md"

echo "Analysis saved to: $OUTPUT_DIR/summary.md"
echo "Raw metrics: $OUTPUT_DIR/metrics.csv"
