#!/bin/bash
# Package experiment data into reproducible archives
# Each archive contains: logs, workflow, metrics summary, and metadata

set -e

EXPERIMENTS_DIR="experiments"
mkdir -p "$EXPERIMENTS_DIR"

package_experiment() {
    local prefix="$1"
    local name="$2"
    local workflow="$3"
    
    local archive_dir="$EXPERIMENTS_DIR/${prefix}_$(date +%Y%m%d_%H%M%S)"
    mkdir -p "$archive_dir"
    
    echo "Packaging experiment: $name ($prefix)"
    
    # Copy logs
    mkdir -p "$archive_dir/logs"
    for node in 1 2 3 4 5 6 7 8 9 10; do
        src="data/${prefix}-${node}/logs/${prefix}-${node}.log"
        if [ -f "$src" ]; then
            cp "$src" "$archive_dir/logs/"
        fi
    done
    
    # Copy workflow if exists
    if [ -n "$workflow" ] && [ -f "$workflow" ]; then
        cp "$workflow" "$archive_dir/"
    fi
    
    # Generate metrics summary
    cat > "$archive_dir/metrics_summary.txt" << EOF
Experiment: $name
Prefix: $prefix
Date: $(date -u +"%Y-%m-%dT%H:%M:%SZ")
Git commit: $(git rev-parse HEAD 2>/dev/null || echo "unknown")
Git branch: $(git branch --show-current 2>/dev/null || echo "unknown")

=== RAW METRICS ===
EOF

    # Extract metrics per node
    for node in 1 2 3; do
        log="data/${prefix}-${node}/logs/${prefix}-${node}.log"
        if [ -f "$log" ]; then
            echo "" >> "$archive_dir/metrics_summary.txt"
            echo "--- Node $node ---" >> "$archive_dir/metrics_summary.txt"
            
            # Sync counts
            syncs=$(grep -c "Sync finished successfully" "$log" 2>/dev/null || echo 0)
            failures=$(grep -c "Sync failed" "$log" 2>/dev/null || echo 0)
            merges=$(grep -c "Concurrent branch detected" "$log" 2>/dev/null || echo 0)
            timeouts=$(grep -c "timeout" "$log" 2>/dev/null || echo 0)
            
            echo "Syncs: $syncs" >> "$archive_dir/metrics_summary.txt"
            echo "Failures: $failures" >> "$archive_dir/metrics_summary.txt"
            echo "Merges: $merges" >> "$archive_dir/metrics_summary.txt"
            echo "Timeouts: $timeouts" >> "$archive_dir/metrics_summary.txt"
            
            # Duration distribution
            echo "" >> "$archive_dir/metrics_summary.txt"
            echo "Duration distribution (ms):" >> "$archive_dir/metrics_summary.txt"
            grep "Sync finished successfully" "$log" 2>/dev/null | \
                grep -oE 'duration_ms="[0-9.]+' | cut -d'"' -f2 | \
                sort -n > "$archive_dir/logs/node${node}_durations.txt"
            
            if [ -s "$archive_dir/logs/node${node}_durations.txt" ]; then
                count=$(wc -l < "$archive_dir/logs/node${node}_durations.txt" | tr -d ' ')
                min=$(head -1 "$archive_dir/logs/node${node}_durations.txt")
                max=$(tail -1 "$archive_dir/logs/node${node}_durations.txt")
                p50_idx=$(( (count + 1) / 2 ))
                p95_idx=$(( (count * 95 + 99) / 100 ))
                p99_idx=$(( (count * 99 + 99) / 100 ))
                p50=$(sed -n "${p50_idx}p" "$archive_dir/logs/node${node}_durations.txt")
                p95=$(sed -n "${p95_idx}p" "$archive_dir/logs/node${node}_durations.txt")
                p99=$(sed -n "${p99_idx}p" "$archive_dir/logs/node${node}_durations.txt")
                
                echo "  Count: $count" >> "$archive_dir/metrics_summary.txt"
                echo "  Min: $min" >> "$archive_dir/metrics_summary.txt"
                echo "  Max: $max" >> "$archive_dir/metrics_summary.txt"
                echo "  P50: $p50" >> "$archive_dir/metrics_summary.txt"
                echo "  P95: $p95" >> "$archive_dir/metrics_summary.txt"
                echo "  P99: $p99" >> "$archive_dir/metrics_summary.txt"
            fi
        fi
    done
    
    # Add instrumentation gaps note
    cat >> "$archive_dir/metrics_summary.txt" << 'EOF'

=== INSTRUMENTATION GAPS ===
The following metrics are NOT available in current logs:
1. Per-phase timing (key_share_ms, data_transfer_ms, merge_ms)
2. Hash comparison count and duration
3. CRDT merge operation count and duration
4. Network bytes sent/received per sync
5. Per-round attribution in multi-round syncs
6. Gossip propagation delay

See MISSING_INSTRUMENTATION.md for required additions.
EOF

    # Create zip
    local zipfile="$EXPERIMENTS_DIR/${prefix}_$(date +%Y%m%d).zip"
    (cd "$archive_dir" && zip -r "../$(basename $zipfile)" .)
    
    echo "Created: $zipfile"
    
    # Cleanup temp dir
    rm -rf "$archive_dir"
}

# Package all available experiments
echo "=== Packaging Experiment Archives ==="
echo ""

package_experiment "b3n10d" "3-Node 10-Key Disjoint" "workflows/sync/bench-3n-10k-disjoint.yml"
package_experiment "b3n50c" "3-Node 50-Key Conflicts" "workflows/sync/bench-3n-50k-conflicts.yml"
package_experiment "b3nlj" "3-Node Late Joiner" "workflows/sync/bench-3n-late-joiner.yml"
package_experiment "b3nrc" "3-Node Restart Catchup" "workflows/sync/bench-3n-restart-catchup.yml"
package_experiment "bench-snap" "Fresh Node Snapshot" "workflows/sync/bench-fresh-node-snapshot.yml"
package_experiment "bench-delta" "Fresh Node Delta" "workflows/sync/bench-fresh-node-delta.yml"
package_experiment "cw" "Continuous Write Stress" "workflows/sync/bench-continuous-write.yml"
package_experiment "lww-node" "LWW Conflict Resolution" "workflows/sync/lww-conflict-resolution.yml"

echo ""
echo "=== Archives Created ==="
ls -la "$EXPERIMENTS_DIR"/*.zip 2>/dev/null || echo "No archives created"
