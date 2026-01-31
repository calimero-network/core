#!/bin/bash
# ============================================================================
# Extract Sync Metrics from Node Logs (Enhanced with Phase Breakdown)
# ============================================================================
#
# Usage: ./scripts/extract-sync-metrics.sh <data_dir_prefix>
# Example: ./scripts/extract-sync-metrics.sh b3n10d
#
# Extracts:
#   - Per-phase timing breakdown (peer_selection, key_share, dag_compare, etc.)
#   - Delta apply timing (WASM execution, merge detection)
#   - Overall sync duration statistics (min, max, avg, p50, p95)
#   - Protocol usage distribution
#
# New log markers parsed:
#   - SYNC_PHASE_BREAKDOWN: Per-phase timing for each sync
#   - DELTA_APPLY_TIMING: Per-delta apply timing with merge detection
#
# ============================================================================

set -e

PREFIX="${1:-b3n10d}"
DATA_DIR="/Users/xilosada/dev/calimero/core/data"
OUTPUT_DIR="$DATA_DIR/${PREFIX}_metrics"

mkdir -p "$OUTPUT_DIR"

echo "=== Sync Metrics for: $PREFIX ==="
echo "Output directory: $OUTPUT_DIR"
echo ""

# ============================================================================
# Phase 1: Extract SYNC_PHASE_BREAKDOWN metrics
# ============================================================================

echo ">>> Extracting SYNC_PHASE_BREAKDOWN..."

# Create temp files for phase data
PEER_SELECTION_FILE=$(mktemp)
KEY_SHARE_FILE=$(mktemp)
DAG_COMPARE_FILE=$(mktemp)
DATA_TRANSFER_FILE=$(mktemp)
TOTAL_SYNC_FILE=$(mktemp)

for node_dir in "$DATA_DIR"/${PREFIX}-*/; do
    if [[ -d "$node_dir" ]]; then
        node_name=$(basename "$node_dir")
        log_file="$node_dir/logs/${node_name}.log"
        
        if [[ -f "$log_file" ]]; then
            # Extract peer_selection_ms
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" | \
                grep -oE 'peer_selection_ms="[0-9.]+"' | \
                sed 's/peer_selection_ms="//;s/"//' >> "$PEER_SELECTION_FILE" 2>/dev/null || true
            
            # Extract key_share_ms
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" | \
                grep -oE 'key_share_ms="[0-9.]+"' | \
                sed 's/key_share_ms="//;s/"//' >> "$KEY_SHARE_FILE" 2>/dev/null || true
            
            # Extract dag_compare_ms
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" | \
                grep -oE 'dag_compare_ms="[0-9.]+"' | \
                sed 's/dag_compare_ms="//;s/"//' >> "$DAG_COMPARE_FILE" 2>/dev/null || true
            
            # Extract data_transfer_ms
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" | \
                grep -oE 'data_transfer_ms="[0-9.]+"' | \
                sed 's/data_transfer_ms="//;s/"//' >> "$DATA_TRANSFER_FILE" 2>/dev/null || true
            
            # Extract total_ms
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" | \
                grep -oE 'total_ms="[0-9.]+"' | \
                sed 's/total_ms="//;s/"//' >> "$TOTAL_SYNC_FILE" 2>/dev/null || true
        fi
    fi
done

# Function to calculate stats
calc_stats() {
    local file="$1"
    local name="$2"
    
    local sorted=$(sort -n "$file" 2>/dev/null | grep -v '^$')
    local count=$(echo "$sorted" | grep -c . 2>/dev/null || echo "0")
    
    if [[ "$count" -gt 0 ]]; then
        local min=$(echo "$sorted" | head -1)
        local max=$(echo "$sorted" | tail -1)
        local sum=$(echo "$sorted" | awk '{sum+=$1} END {print sum}')
        local avg=$(echo "scale=2; $sum / $count" | bc)
        
        local p50_idx=$(echo "$count * 50 / 100" | bc)
        local p95_idx=$(echo "$count * 95 / 100" | bc)
        [[ "$p50_idx" -lt 1 ]] && p50_idx=1
        [[ "$p95_idx" -lt 1 ]] && p95_idx=1
        
        local p50=$(echo "$sorted" | sed -n "${p50_idx}p")
        local p95=$(echo "$sorted" | sed -n "${p95_idx}p")
        
        echo "$name:"
        echo "  Count: $count"
        echo "  Min:   ${min}ms"
        echo "  Max:   ${max}ms"
        echo "  Avg:   ${avg}ms"
        echo "  P50:   ${p50}ms"
        echo "  P95:   ${p95}ms"
        echo ""
        
        # Save to file
        echo "$name,$count,$min,$max,$avg,$p50,$p95" >> "$OUTPUT_DIR/phase_stats.csv"
    else
        echo "$name: No data"
        echo ""
    fi
}

# Initialize CSV
echo "phase,count,min_ms,max_ms,avg_ms,p50_ms,p95_ms" > "$OUTPUT_DIR/phase_stats.csv"

echo ""
echo "=== PER-PHASE TIMING STATISTICS ==="
echo ""

calc_stats "$PEER_SELECTION_FILE" "peer_selection"
calc_stats "$KEY_SHARE_FILE" "key_share"
calc_stats "$DAG_COMPARE_FILE" "dag_compare"
calc_stats "$DATA_TRANSFER_FILE" "data_transfer"
calc_stats "$TOTAL_SYNC_FILE" "total_sync"

# Cleanup temp files
rm -f "$PEER_SELECTION_FILE" "$KEY_SHARE_FILE" "$DAG_COMPARE_FILE" "$DATA_TRANSFER_FILE" "$TOTAL_SYNC_FILE"

# ============================================================================
# Phase 2: Extract DELTA_APPLY_TIMING metrics
# ============================================================================

echo ">>> Extracting DELTA_APPLY_TIMING..."

WASM_TIME_FILE=$(mktemp)
DELTA_TOTAL_FILE=$(mktemp)
MERGE_COUNT=0
NON_MERGE_COUNT=0

for node_dir in "$DATA_DIR"/${PREFIX}-*/; do
    if [[ -d "$node_dir" ]]; then
        node_name=$(basename "$node_dir")
        log_file="$node_dir/logs/${node_name}.log"
        
        if [[ -f "$log_file" ]]; then
            # Extract wasm_ms
            grep "DELTA_APPLY_TIMING" "$log_file" | \
                grep -oE 'wasm_ms="[0-9.]+"' | \
                sed 's/wasm_ms="//;s/"//' >> "$WASM_TIME_FILE" 2>/dev/null || true
            
            # Extract total_ms for delta apply
            grep "DELTA_APPLY_TIMING" "$log_file" | \
                grep -oE 'total_ms="[0-9.]+"' | \
                sed 's/total_ms="//;s/"//' >> "$DELTA_TOTAL_FILE" 2>/dev/null || true
            
            # Count merges (use grep -o to avoid counting lines)
            merges=$(grep -o "was_merge=true" "$log_file" 2>/dev/null | wc -l | tr -d ' ')
            non_merges=$(grep -o "was_merge=false" "$log_file" 2>/dev/null | wc -l | tr -d ' ')
            [[ -z "$merges" || ! "$merges" =~ ^[0-9]+$ ]] && merges=0
            [[ -z "$non_merges" || ! "$non_merges" =~ ^[0-9]+$ ]] && non_merges=0
            MERGE_COUNT=$((MERGE_COUNT + merges))
            NON_MERGE_COUNT=$((NON_MERGE_COUNT + non_merges))
        fi
    fi
done

echo ""
echo "=== DELTA APPLY TIMING STATISTICS ==="
echo ""

echo "delta_wasm_exec,$count,$min,$max,$avg,$p50,$p95" >> "$OUTPUT_DIR/phase_stats.csv"
echo "delta_total,$count,$min,$max,$avg,$p50,$p95" >> "$OUTPUT_DIR/phase_stats.csv"

calc_stats "$WASM_TIME_FILE" "delta_wasm_exec"
calc_stats "$DELTA_TOTAL_FILE" "delta_total"

echo "Merge Statistics:"
echo "  Deltas with merge: $MERGE_COUNT"
echo "  Deltas without merge: $NON_MERGE_COUNT"
TOTAL_DELTAS=$((MERGE_COUNT + NON_MERGE_COUNT))
if [[ "$TOTAL_DELTAS" -gt 0 ]]; then
    MERGE_RATIO=$(echo "scale=2; $MERGE_COUNT * 100 / $TOTAL_DELTAS" | bc)
    echo "  Merge ratio: ${MERGE_RATIO}%"
fi
echo ""

rm -f "$WASM_TIME_FILE" "$DELTA_TOTAL_FILE"

# ============================================================================
# Phase 3: Extract protocol distribution
# ============================================================================

echo ">>> Extracting protocol distribution..."
echo ""
echo "=== PROTOCOL USAGE ==="
echo ""

PROTOCOL_FILE=$(mktemp)
for node_dir in "$DATA_DIR"/${PREFIX}-*/; do
    if [[ -d "$node_dir" ]]; then
        node_name=$(basename "$node_dir")
        log_file="$node_dir/logs/${node_name}.log"
        
        if [[ -f "$log_file" ]]; then
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" | \
                grep -oE 'protocol=[A-Za-z]+' | \
                sed 's/protocol=//' >> "$PROTOCOL_FILE" 2>/dev/null || true
        fi
    fi
done

if [[ -s "$PROTOCOL_FILE" ]]; then
    echo "Protocol Distribution:"
    sort "$PROTOCOL_FILE" | uniq -c | sort -rn | while read count proto; do
        echo "  $proto: $count"
    done
else
    echo "  No protocol data found"
fi
echo ""

rm -f "$PROTOCOL_FILE"

# ============================================================================
# Phase 4: Summary with P95/P50 ratio analysis
# ============================================================================

echo "=== TAIL LATENCY ANALYSIS ==="
echo ""

# Read back phase stats and analyze
if [[ -f "$OUTPUT_DIR/phase_stats.csv" ]]; then
    while IFS=, read -r phase count min_ms max_ms avg_ms p50_ms p95_ms; do
        [[ "$phase" == "phase" ]] && continue  # Skip header
        [[ -z "$p50_ms" || -z "$p95_ms" ]] && continue
        
        # Calculate P95/P50 ratio
        ratio=$(echo "scale=2; $p95_ms / $p50_ms" | bc 2>/dev/null || echo "N/A")
        
        if [[ "$ratio" != "N/A" ]]; then
            # Flag if P95 > 2x P50 (indicates tail latency issue)
            is_problem=""
            ratio_check=$(echo "$ratio > 2" | bc 2>/dev/null || echo "0")
            if [[ "$ratio_check" == "1" ]]; then
                is_problem=" ⚠️  TAIL LATENCY ISSUE"
            fi
            
            echo "$phase: P95/P50 = ${ratio}x${is_problem}"
        fi
    done < "$OUTPUT_DIR/phase_stats.csv"
fi

echo ""
echo "=== SUMMARY ==="
echo ""
echo "Phase breakdown saved to: $OUTPUT_DIR/phase_stats.csv"
echo ""

# ============================================================================
# Phase 5: Generate human-readable summary
# ============================================================================

{
    echo "# Sync Metrics Summary for: $PREFIX"
    echo "Generated: $(date)"
    echo ""
    echo "## Phase Timing"
    echo ""
    echo "| Phase | Count | P50 (ms) | P95 (ms) | P95/P50 |"
    echo "|-------|-------|----------|----------|---------|"
    
    while IFS=, read -r phase count min_ms max_ms avg_ms p50_ms p95_ms; do
        [[ "$phase" == "phase" ]] && continue
        [[ -z "$p50_ms" || -z "$p95_ms" ]] && continue
        ratio=$(echo "scale=2; $p95_ms / $p50_ms" | bc 2>/dev/null || echo "N/A")
        echo "| $phase | $count | $p50_ms | $p95_ms | ${ratio}x |"
    done < "$OUTPUT_DIR/phase_stats.csv"
    
    echo ""
    echo "## Delta Application"
    echo ""
    echo "- Deltas with merge: $MERGE_COUNT"
    echo "- Deltas without merge: $NON_MERGE_COUNT"
    echo "- Merge ratio: ${MERGE_RATIO:-N/A}%"
    echo ""
} > "$OUTPUT_DIR/summary.md"

cat "$OUTPUT_DIR/summary.md"

echo ""
echo "=== DONE ==="
echo "Full summary at: $OUTPUT_DIR/summary.md"
