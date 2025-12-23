#!/bin/bash
# Collect profiling data from Docker containers
# Usage: collect-from-containers.sh <test-name> <logs-dir> <data-dir> <reports-dir>
#
# Arguments:
#   test-name    Name of the test (e.g., kv-store, kv-store-with-handlers)
#   logs-dir     Directory for container logs
#   data-dir     Directory for profiling data
#   reports-dir  Directory for profiling reports

set -e

TEST_NAME="${1:?Error: test-name is required}"
LOGS_DIR="${2:?Error: logs-dir is required}"
DATA_DIR="${3:?Error: data-dir is required}"
REPORTS_DIR="${4:?Error: reports-dir is required}"

echo "Collecting profiling data from Docker containers..."
echo "  Test:    $TEST_NAME"
echo "  Logs:    $LOGS_DIR"
echo "  Data:    $DATA_DIR"
echo "  Reports: $REPORTS_DIR"

# Find all fuzzy test containers
for container in $(docker ps -a --filter "label=calimero.node=true" --format "{{.Names}}" 2>/dev/null || true); do
    if [ -n "$container" ]; then
        echo ""
        echo "Collecting from container: $container"
        
        # Create container-specific directory
        mkdir -p "$DATA_DIR/$container"
        mkdir -p "$REPORTS_DIR/$container"
        
        # Copy profiling data from container
        docker cp "$container:/profiling/data/." "$DATA_DIR/$container/" 2>/dev/null || echo "  No profiling data in $container"
        docker cp "$container:/profiling/reports/." "$REPORTS_DIR/$container/" 2>/dev/null || echo "  No profiling reports in $container"
        
        # Collect container logs
        docker logs "$container" > "$LOGS_DIR/${container}.log" 2>&1 || true
        echo "  Collected container logs"
        
        # Try to generate flamegraph if perf data exists
        # Find the actual perf data file (could be perf-merod.data, perf-node1.data, etc.)
        PERF_FILE=$(ls "$DATA_DIR/$container"/perf-*.data 2>/dev/null | head -1)
        if [ -n "$PERF_FILE" ]; then
            # Extract just the filename for use inside the container
            PERF_BASENAME=$(basename "$PERF_FILE")
            echo "  Found perf data: $PERF_BASENAME"
            echo "  Generating flamegraph for $container..."
            docker exec "$container" /profiling/scripts/generate-flamegraph.sh \
                --input "/profiling/data/$PERF_BASENAME" \
                --output /profiling/reports/flamegraph.svg \
                --title "CPU Flamegraph - $container" 2>/dev/null || echo "  Could not generate flamegraph"
            
            # Copy generated reports
            docker cp "$container:/profiling/reports/." "$REPORTS_DIR/$container/" 2>/dev/null || true
        fi
        
        # Generate memory report (use container name for identification)
        docker exec "$container" /profiling/scripts/generate-memory-report.sh \
            --node-name "$container" \
            --output /profiling/reports/memory-report.txt 2>/dev/null || echo "  Could not generate memory report"
        docker cp "$container:/profiling/reports/memory-report.txt" "$REPORTS_DIR/$container/" 2>/dev/null || true
    fi
done

echo ""
echo "Profiling data collection complete for $TEST_NAME"

