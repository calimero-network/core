#!/bin/bash
# ============================================================================
# Extract Sync Metrics from Node Logs
# ============================================================================
#
# Parses the following log markers:
# - SYNC_PHASE_BREAKDOWN: Per-phase timing for sync operations
# - DELTA_APPLY_TIMING: WASM execution and delta application timing
# - STRATEGY_SYNC_METRICS: State sync strategy performance
# - PEER_FIND_BREAKDOWN: Peer finding/discovery timing (NEW)
#
# ============================================================================
# ============================================================================
# Extract Sync Metrics from Node Logs (Enhanced with Strategy Metrics)
# ============================================================================
#
# Usage: ./scripts/extract-sync-metrics.sh <data_dir_prefix>
# Example: ./scripts/extract-sync-metrics.sh bloom
#
# Extracts:
#   - Strategy-specific metrics (STRATEGY_SYNC_METRICS)
#   - Per-phase timing breakdown (SYNC_PHASE_BREAKDOWN)
#   - Delta apply timing (DELTA_APPLY_TIMING)
#   - Overall sync duration statistics (min, max, avg, p50, p95)
#   - Protocol usage distribution
#
# New log markers parsed:
#   - STRATEGY_SYNC_METRICS: Per-strategy performance data
#   - SYNC_PHASE_BREAKDOWN: Per-phase timing for each sync
#   - DELTA_APPLY_TIMING: Per-delta apply timing with merge detection
#
# ============================================================================

set -e

PREFIX="${1:-bloom}"
DATA_DIR="${2:-/Users/xilosada/dev/calimero/core/data}"
OUTPUT_DIR="$DATA_DIR/${PREFIX}_metrics"

mkdir -p "$OUTPUT_DIR"

echo "=== Sync Metrics for: $PREFIX ==="
echo "Output directory: $OUTPUT_DIR"
echo ""

# ============================================================================
# Phase 0: Extract STRATEGY_SYNC_METRICS (New!)
# ============================================================================

echo ">>> Extracting STRATEGY_SYNC_METRICS..."

# Create temp files for strategy data
BLOOM_FILE=$(mktemp)
HASH_FILE=$(mktemp)
SUBTREE_FILE=$(mktemp)
LEVEL_FILE=$(mktemp)

# Track strategy counts
BLOOM_COUNT=0
HASH_COUNT=0
SUBTREE_COUNT=0
LEVEL_COUNT=0

for node_dir in "$DATA_DIR"/${PREFIX}-*/; do
    if [[ -d "$node_dir" ]]; then
        node_name=$(basename "$node_dir")
        log_file="$node_dir/logs/${node_name}.log"
        
        if [[ -f "$log_file" ]]; then
            # Extract bloom_filter metrics
            grep "STRATEGY_SYNC_METRICS" "$log_file" 2>/dev/null | \
                grep 'strategy="bloom_filter"' | while read line; do
                    round_trips=$(echo "$line" | grep -oE 'round_trips=[0-9]+' | sed 's/round_trips=//')
                    entities_synced=$(echo "$line" | grep -oE 'entities_synced=[0-9]+' | sed 's/entities_synced=//')
                    bytes_received=$(echo "$line" | grep -oE 'bytes_received=[0-9]+' | sed 's/bytes_received=//')
                    bytes_sent=$(echo "$line" | grep -oE 'bytes_sent=[0-9]+' | sed 's/bytes_sent=//')
                    duration_ms=$(echo "$line" | grep -oE 'duration_ms="[0-9.]+"' | sed 's/duration_ms="//;s/"//')
                    bloom_filter_size=$(echo "$line" | grep -oE 'bloom_filter_size=[0-9]+' | sed 's/bloom_filter_size=//')
                    matched_count=$(echo "$line" | grep -oE 'matched_count=[0-9]+' | sed 's/matched_count=//')
                    
                    [[ -n "$duration_ms" ]] && echo "$node_name,$duration_ms,$round_trips,$entities_synced,$bytes_received,$bytes_sent,$bloom_filter_size,$matched_count" >> "$BLOOM_FILE"
                done
            
            # Extract hash_comparison metrics
            grep "STRATEGY_SYNC_METRICS" "$log_file" 2>/dev/null | \
                grep 'strategy="hash_comparison"' | while read line; do
                    round_trips=$(echo "$line" | grep -oE 'round_trips=[0-9]+' | sed 's/round_trips=//')
                    entities_synced=$(echo "$line" | grep -oE 'entities_synced=[0-9]+' | sed 's/entities_synced=//')
                    bytes_received=$(echo "$line" | grep -oE 'bytes_received=[0-9]+' | sed 's/bytes_received=//')
                    duration_ms=$(echo "$line" | grep -oE 'duration_ms="[0-9.]+"' | sed 's/duration_ms="//;s/"//')
                    nodes_checked=$(echo "$line" | grep -oE 'nodes_checked=[0-9]+' | sed 's/nodes_checked=//')
                    max_depth=$(echo "$line" | grep -oE 'max_depth_reached=[0-9]+' | sed 's/max_depth_reached=//')
                    hash_comparisons=$(echo "$line" | grep -oE 'hash_comparisons=[0-9]+' | sed 's/hash_comparisons=//')
                    
                    [[ -n "$duration_ms" ]] && echo "$node_name,$duration_ms,$round_trips,$entities_synced,$bytes_received,$nodes_checked,$max_depth,$hash_comparisons" >> "$HASH_FILE"
                done
            
            # Extract subtree_prefetch metrics
            grep "STRATEGY_SYNC_METRICS" "$log_file" 2>/dev/null | \
                grep 'strategy="subtree_prefetch"' | while read line; do
                    round_trips=$(echo "$line" | grep -oE 'round_trips=[0-9]+' | sed 's/round_trips=//')
                    entities_synced=$(echo "$line" | grep -oE 'entities_synced=[0-9]+' | sed 's/entities_synced=//')
                    bytes_received=$(echo "$line" | grep -oE 'bytes_received=[0-9]+' | sed 's/bytes_received=//')
                    duration_ms=$(echo "$line" | grep -oE 'duration_ms="[0-9.]+"' | sed 's/duration_ms="//;s/"//')
                    subtrees_fetched=$(echo "$line" | grep -oE 'subtrees_fetched=[0-9]+' | sed 's/subtrees_fetched=//')
                    divergent_children=$(echo "$line" | grep -oE 'divergent_children=[0-9]+' | sed 's/divergent_children=//')
                    
                    [[ -n "$duration_ms" ]] && echo "$node_name,$duration_ms,$round_trips,$entities_synced,$bytes_received,$subtrees_fetched,$divergent_children" >> "$SUBTREE_FILE"
                done
            
            # Extract level_wise metrics
            grep "STRATEGY_SYNC_METRICS" "$log_file" 2>/dev/null | \
                grep 'strategy="level_wise"' | while read line; do
                    round_trips=$(echo "$line" | grep -oE 'round_trips=[0-9]+' | sed 's/round_trips=//')
                    entities_synced=$(echo "$line" | grep -oE 'entities_synced=[0-9]+' | sed 's/entities_synced=//')
                    bytes_received=$(echo "$line" | grep -oE 'bytes_received=[0-9]+' | sed 's/bytes_received=//')
                    duration_ms=$(echo "$line" | grep -oE 'duration_ms="[0-9.]+"' | sed 's/duration_ms="//;s/"//')
                    levels_synced=$(echo "$line" | grep -oE 'levels_synced=[0-9]+' | sed 's/levels_synced=//')
                    max_nodes_per_level=$(echo "$line" | grep -oE 'max_nodes_per_level=[0-9]+' | sed 's/max_nodes_per_level=//')
                    
                    [[ -n "$duration_ms" ]] && echo "$node_name,$duration_ms,$round_trips,$entities_synced,$bytes_received,$levels_synced,$max_nodes_per_level" >> "$LEVEL_FILE"
                done
        fi
    fi
done

# Function to calculate stats for a column in CSV
calc_column_stats() {
    local file="$1"
    local col="$2"  # 1-indexed column
    local name="$3"
    
    if [[ ! -s "$file" ]]; then
        echo "$name: No data"
        return
    fi
    
    local sorted=$(cut -d',' -f"$col" "$file" | sort -n 2>/dev/null | grep -v '^$')
    local count=$(echo "$sorted" | grep -c . 2>/dev/null || echo "0")
    
    if [[ "$count" -gt 0 ]]; then
        local min=$(echo "$sorted" | head -1)
        local max=$(echo "$sorted" | tail -1)
        local sum=$(echo "$sorted" | awk '{sum+=$1} END {print sum}')
        local avg=$(echo "scale=2; $sum / $count" | bc 2>/dev/null || echo "0")
        
        local p50_idx=$(echo "$count * 50 / 100" | bc)
        local p95_idx=$(echo "$count * 95 / 100" | bc)
        [[ "$p50_idx" -lt 1 ]] && p50_idx=1
        [[ "$p95_idx" -lt 1 ]] && p95_idx=1
        
        local p50=$(echo "$sorted" | sed -n "${p50_idx}p")
        local p95=$(echo "$sorted" | sed -n "${p95_idx}p")
        
        echo "$name: n=$count, min=${min}, max=${max}, avg=${avg}, p50=${p50}, p95=${p95}"
    else
        echo "$name: No data"
    fi
}

echo ""
echo "=== STRATEGY-SPECIFIC METRICS ==="
echo ""

# Bloom Filter stats
BLOOM_COUNT=$(wc -l < "$BLOOM_FILE" 2>/dev/null | tr -d ' ')
[[ -z "$BLOOM_COUNT" || ! "$BLOOM_COUNT" =~ ^[0-9]+$ ]] && BLOOM_COUNT=0
if [[ "$BLOOM_COUNT" -gt 0 ]]; then
    echo "--- Bloom Filter Strategy ---"
    echo "Syncs: $BLOOM_COUNT"
    calc_column_stats "$BLOOM_FILE" 2 "Duration (ms)"
    calc_column_stats "$BLOOM_FILE" 3 "Round trips"
    calc_column_stats "$BLOOM_FILE" 4 "Entities synced"
    calc_column_stats "$BLOOM_FILE" 5 "Bytes received"
    calc_column_stats "$BLOOM_FILE" 7 "Filter size"
    echo ""
fi

# Hash Comparison stats
HASH_COUNT=$(wc -l < "$HASH_FILE" 2>/dev/null | tr -d ' ')
[[ -z "$HASH_COUNT" || ! "$HASH_COUNT" =~ ^[0-9]+$ ]] && HASH_COUNT=0
if [[ "$HASH_COUNT" -gt 0 ]]; then
    echo "--- Hash Comparison Strategy ---"
    echo "Syncs: $HASH_COUNT"
    calc_column_stats "$HASH_FILE" 2 "Duration (ms)"
    calc_column_stats "$HASH_FILE" 3 "Round trips"
    calc_column_stats "$HASH_FILE" 4 "Entities synced"
    calc_column_stats "$HASH_FILE" 6 "Nodes checked"
    calc_column_stats "$HASH_FILE" 7 "Max depth"
    calc_column_stats "$HASH_FILE" 8 "Hash comparisons"
    echo ""
fi

# Subtree Prefetch stats
SUBTREE_COUNT=$(wc -l < "$SUBTREE_FILE" 2>/dev/null | tr -d ' ')
[[ -z "$SUBTREE_COUNT" || ! "$SUBTREE_COUNT" =~ ^[0-9]+$ ]] && SUBTREE_COUNT=0
if [[ "$SUBTREE_COUNT" -gt 0 ]]; then
    echo "--- Subtree Prefetch Strategy ---"
    echo "Syncs: $SUBTREE_COUNT"
    calc_column_stats "$SUBTREE_FILE" 2 "Duration (ms)"
    calc_column_stats "$SUBTREE_FILE" 3 "Round trips"
    calc_column_stats "$SUBTREE_FILE" 4 "Entities synced"
    calc_column_stats "$SUBTREE_FILE" 6 "Subtrees fetched"
    calc_column_stats "$SUBTREE_FILE" 7 "Divergent children"
    echo ""
fi

# Level-Wise stats
LEVEL_COUNT=$(wc -l < "$LEVEL_FILE" 2>/dev/null | tr -d ' ')
[[ -z "$LEVEL_COUNT" || ! "$LEVEL_COUNT" =~ ^[0-9]+$ ]] && LEVEL_COUNT=0
if [[ "$LEVEL_COUNT" -gt 0 ]]; then
    echo "--- Level-Wise Strategy ---"
    echo "Syncs: $LEVEL_COUNT"
    calc_column_stats "$LEVEL_FILE" 2 "Duration (ms)"
    calc_column_stats "$LEVEL_FILE" 3 "Round trips"
    calc_column_stats "$LEVEL_FILE" 4 "Entities synced"
    calc_column_stats "$LEVEL_FILE" 6 "Levels synced"
    calc_column_stats "$LEVEL_FILE" 7 "Max nodes/level"
    echo ""
fi

# Save raw data
cp "$BLOOM_FILE" "$OUTPUT_DIR/bloom_filter_raw.csv" 2>/dev/null || true
cp "$HASH_FILE" "$OUTPUT_DIR/hash_comparison_raw.csv" 2>/dev/null || true
cp "$SUBTREE_FILE" "$OUTPUT_DIR/subtree_prefetch_raw.csv" 2>/dev/null || true
cp "$LEVEL_FILE" "$OUTPUT_DIR/level_wise_raw.csv" 2>/dev/null || true

rm -f "$BLOOM_FILE" "$HASH_FILE" "$SUBTREE_FILE" "$LEVEL_FILE"

# ============================================================================
# Phase 1: Extract SYNC_PHASE_BREAKDOWN metrics (existing)
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
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                grep -oE 'peer_selection_ms="[0-9.]+"' | \
                sed 's/peer_selection_ms="//;s/"//' >> "$PEER_SELECTION_FILE" 2>/dev/null || true
            
            # Extract key_share_ms
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                grep -oE 'key_share_ms="[0-9.]+"' | \
                sed 's/key_share_ms="//;s/"//' >> "$KEY_SHARE_FILE" 2>/dev/null || true
            
            # Extract dag_compare_ms
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                grep -oE 'dag_compare_ms="[0-9.]+"' | \
                sed 's/dag_compare_ms="//;s/"//' >> "$DAG_COMPARE_FILE" 2>/dev/null || true
            
            # Extract data_transfer_ms
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                grep -oE 'data_transfer_ms="[0-9.]+"' | \
                sed 's/data_transfer_ms="//;s/"//' >> "$DATA_TRANSFER_FILE" 2>/dev/null || true
            
            # Extract total_ms
            grep "SYNC_PHASE_BREAKDOWN" "$log_file" 2>/dev/null | \
                grep -oE 'total_ms="[0-9.]+"' | \
                sed 's/total_ms="//;s/"//' >> "$TOTAL_SYNC_FILE" 2>/dev/null || true
        fi
    fi
done

# Function to calculate stats
calc_stats() {
    local file="$1"
    local name="$2"
    
    if [[ ! -s "$file" ]]; then
        echo "$name: No data"
        echo ""
        return
    fi
    
    local sorted=$(sort -n "$file" 2>/dev/null | grep -v '^$')
    local count=$(echo "$sorted" | grep -c . 2>/dev/null || echo "0")
    
    if [[ "$count" -gt 0 ]]; then
        local min=$(echo "$sorted" | head -1)
        local max=$(echo "$sorted" | tail -1)
        local sum=$(echo "$sorted" | awk '{sum+=$1} END {print sum}')
        local avg=$(echo "scale=2; $sum / $count" | bc 2>/dev/null || echo "0")
        
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
            grep "DELTA_APPLY_TIMING" "$log_file" 2>/dev/null | \
                grep -oE 'wasm_ms="[0-9.]+"' | \
                sed 's/wasm_ms="//;s/"//' >> "$WASM_TIME_FILE" 2>/dev/null || true
            
            # Extract total_ms for delta apply
            grep "DELTA_APPLY_TIMING" "$log_file" 2>/dev/null | \
                grep -oE 'total_ms="[0-9.]+"' | \
                sed 's/total_ms="//;s/"//' >> "$DELTA_TOTAL_FILE" 2>/dev/null || true
            
            # Count merges
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

calc_stats "$WASM_TIME_FILE" "delta_wasm_exec"
calc_stats "$DELTA_TOTAL_FILE" "delta_total"

echo "Merge Statistics:"
echo "  Deltas with merge: $MERGE_COUNT"
echo "  Deltas without merge: $NON_MERGE_COUNT"
TOTAL_DELTAS=$((MERGE_COUNT + NON_MERGE_COUNT))
if [[ "$TOTAL_DELTAS" -gt 0 ]]; then
    MERGE_RATIO=$(echo "scale=2; $MERGE_COUNT * 100 / $TOTAL_DELTAS" | bc 2>/dev/null || echo "0")
    echo "  Merge ratio: ${MERGE_RATIO}%"
fi
echo ""

rm -f "$WASM_TIME_FILE" "$DELTA_TOTAL_FILE"

# ============================================================================
# Phase 3: Strategy Comparison Summary
# ============================================================================

echo "=== STRATEGY COMPARISON SUMMARY ==="
echo ""

echo "| Strategy | Syncs | Avg Duration | Avg Round Trips | Avg Entities |"
echo "|----------|-------|--------------|-----------------|--------------|"

for strategy in bloom_filter hash_comparison subtree_prefetch level_wise; do
    file="$OUTPUT_DIR/${strategy}_raw.csv"
    if [[ -s "$file" ]]; then
        count=$(wc -l < "$file" | tr -d ' ')
        avg_duration=$(cut -d',' -f2 "$file" | awk '{sum+=$1;count++} END {if(count>0) printf "%.2f", sum/count; else print "N/A"}')
        avg_round_trips=$(cut -d',' -f3 "$file" | awk '{sum+=$1;count++} END {if(count>0) printf "%.1f", sum/count; else print "N/A"}')
        avg_entities=$(cut -d',' -f4 "$file" | awk '{sum+=$1;count++} END {if(count>0) printf "%.1f", sum/count; else print "N/A"}')
        echo "| $strategy | $count | ${avg_duration}ms | $avg_round_trips | $avg_entities |"
    fi
done

echo ""

# ============================================================================
# Phase 4: Extract PEER_FIND_BREAKDOWN metrics
# ============================================================================

echo ">>> Extracting PEER_FIND_BREAKDOWN..."

PEER_FIND_FILE=$(mktemp)

for node_dir in "$DATA_DIR"/${PREFIX}-*/; do
    if [[ -d "$node_dir" ]]; then
        node_name=$(basename "$node_dir")
        log_file="$node_dir/logs/${node_name}.log"
        
        if [[ -f "$log_file" ]]; then
            # Extract peer find breakdown data
            grep "PEER_FIND_BREAKDOWN" "$log_file" 2>/dev/null | while IFS= read -r line; do
                total_ms=$(echo "$line" | grep -oE 'peer_find_total_ms=[0-9.]+' | cut -d'=' -f2)
                from_mesh_ms=$(echo "$line" | grep -oE 'from_mesh_ms=[0-9.]+' | cut -d'=' -f2)
                candidates_total=$(echo "$line" | grep -oE 'candidates_total=[0-9]+' | cut -d'=' -f2)
                candidates_mesh=$(echo "$line" | grep -oE 'candidates_from_mesh=[0-9]+' | cut -d'=' -f2)
                selected_source=$(echo "$line" | grep -oE 'selected_peer_source=[a-z]+' | cut -d'=' -f2)
                
                if [[ -n "$total_ms" ]]; then
                    echo "${total_ms},${from_mesh_ms:-0},${candidates_total:-0},${candidates_mesh:-0},${selected_source:-unknown}" >> "$PEER_FIND_FILE"
                fi
            done
        fi
    fi
done

echo ""
echo "=== PEER FINDING METRICS ==="
echo ""

PEER_FIND_COUNT=$(wc -l < "$PEER_FIND_FILE" 2>/dev/null | tr -d ' ')
[[ -z "$PEER_FIND_COUNT" || ! "$PEER_FIND_COUNT" =~ ^[0-9]+$ ]] && PEER_FIND_COUNT=0

if [[ "$PEER_FIND_COUNT" -gt 0 ]]; then
    echo "Total peer find attempts: $PEER_FIND_COUNT"
    echo ""
    
    # Extract just the total_ms column for percentile calculation
    PEER_FIND_TOTAL_FILE=$(mktemp)
    cut -d',' -f1 "$PEER_FIND_FILE" > "$PEER_FIND_TOTAL_FILE"
    
    calc_stats "$PEER_FIND_TOTAL_FILE" "peer_find_total_ms"
    
    # Extract mesh timing
    MESH_TIME_FILE=$(mktemp)
    cut -d',' -f2 "$PEER_FIND_FILE" > "$MESH_TIME_FILE"
    calc_stats "$MESH_TIME_FILE" "from_mesh_ms"
    
    # Candidate stats
    AVG_CANDIDATES=$(cut -d',' -f3 "$PEER_FIND_FILE" | awk '{sum+=$1;count++} END {if(count>0) printf "%.1f", sum/count; else print "0"}')
    echo "Avg candidates found: $AVG_CANDIDATES"
    
    # Source distribution
    echo ""
    echo "Selected peer source distribution:"
    cut -d',' -f5 "$PEER_FIND_FILE" | sort | uniq -c | sort -rn
    
    rm -f "$PEER_FIND_TOTAL_FILE" "$MESH_TIME_FILE"
else
    echo "No peer find data found"
fi

cp "$PEER_FIND_FILE" "$OUTPUT_DIR/peer_find_raw.csv" 2>/dev/null || true
rm -f "$PEER_FIND_FILE"

echo ""

# ============================================================================
# Phase 5: Generate summary file
# ============================================================================

{
    echo "# Sync Metrics Summary for: $PREFIX"
    echo "Generated: $(date)"
    echo ""
    echo "## Strategy Performance"
    echo ""
    echo "| Strategy | Syncs | Avg Duration (ms) | Avg Round Trips |"
    echo "|----------|-------|-------------------|-----------------|"
    
    for strategy in bloom_filter hash_comparison subtree_prefetch level_wise; do
        file="$OUTPUT_DIR/${strategy}_raw.csv"
        if [[ -s "$file" ]]; then
            count=$(wc -l < "$file" | tr -d ' ')
            avg_duration=$(cut -d',' -f2 "$file" | awk '{sum+=$1;count++} END {if(count>0) printf "%.2f", sum/count; else print "N/A"}')
            avg_round_trips=$(cut -d',' -f3 "$file" | awk '{sum+=$1;count++} END {if(count>0) printf "%.1f", sum/count; else print "N/A"}')
            echo "| $strategy | $count | $avg_duration | $avg_round_trips |"
        fi
    done
    
    echo ""
    echo "## Delta Application"
    echo ""
    echo "- Deltas with merge: $MERGE_COUNT"
    echo "- Deltas without merge: $NON_MERGE_COUNT"
    echo "- Merge ratio: ${MERGE_RATIO:-N/A}%"
    echo ""
} > "$OUTPUT_DIR/summary.md"

echo "=== DONE ==="
echo "Full summary at: $OUTPUT_DIR/summary.md"
echo "Raw data at: $OUTPUT_DIR/"