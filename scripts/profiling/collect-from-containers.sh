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
        
        # CRITICAL: Stop perf gracefully BEFORE copying data
        # perf buffers samples and only writes them when stopped with SIGINT
        echo "  Stopping perf profiler to flush data..."
        docker exec "$container" bash -c '
            if [ -f /profiling/data/perf.pid ]; then
                PERF_PID=$(cat /profiling/data/perf.pid)
                if kill -0 "$PERF_PID" 2>/dev/null; then
                    echo "    Sending SIGINT to perf (PID $PERF_PID)..."
                    kill -INT "$PERF_PID" 2>/dev/null || true
                    # Wait for perf to finish writing data
                    for i in $(seq 1 10); do
                        if ! kill -0 "$PERF_PID" 2>/dev/null; then
                            echo "    perf stopped successfully"
                            break
                        fi
                        sleep 1
                    done
                    # Force kill if still running
                    if kill -0 "$PERF_PID" 2>/dev/null; then
                        echo "    WARNING: perf did not stop gracefully, forcing kill"
                        kill -KILL "$PERF_PID" 2>/dev/null || true
                    fi
                else
                    echo "    perf already stopped"
                fi
                rm -f /profiling/data/perf.pid
            else
                echo "    No perf.pid file found"
            fi
        ' 2>/dev/null || echo "  Could not stop perf (container may have already stopped)"
        
        # Small delay to ensure file is fully written
        sleep 1
        
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
            
            # Check file size to verify perf captured data
            FILE_SIZE=$(stat -f%z "$PERF_FILE" 2>/dev/null || stat -c%s "$PERF_FILE" 2>/dev/null || echo "0")
            echo "  Found perf data: $PERF_BASENAME ($FILE_SIZE bytes)"
            
            if [ "$FILE_SIZE" -lt 1000 ]; then
                echo "  WARNING: perf data file is very small ($FILE_SIZE bytes)"
                echo "  This usually means perf didn't collect samples. Possible causes:"
                echo "    - Container was killed before perf was stopped gracefully"
                echo "    - Process had very low CPU usage"
                echo "    - perf wasn't recording successfully"
            fi
            
            echo "  Generating flamegraph for $container..."
            docker exec "$container" /profiling/scripts/generate-flamegraph.sh \
                --input "/profiling/data/$PERF_BASENAME" \
                --output /profiling/reports/flamegraph.svg \
                --title "CPU Flamegraph - $container" 2>/dev/null || echo "  Could not generate flamegraph"
            
            # Copy generated reports
            docker cp "$container:/profiling/reports/." "$REPORTS_DIR/$container/" 2>/dev/null || true
        else
            echo "  No perf data file found"
        fi
        
        # Generate memory report (use container name for identification)
        echo "  Generating memory report..."
        if docker exec "$container" /profiling/scripts/generate-memory-report.sh \
            --node-name "$container" \
            --output /profiling/reports/memory-report.txt 2>&1; then
            docker cp "$container:/profiling/reports/memory-report.txt" "$REPORTS_DIR/$container/" 2>/dev/null && \
                echo "  Memory report saved to $REPORTS_DIR/$container/memory-report.txt" || \
                echo "  WARNING: Could not copy memory report"
        else
            echo "  Could not generate memory report"
        fi
        
        # Generate memory flamegraph from jemalloc heap dumps
        echo "  Generating memory flamegraph..."
        HEAP_DUMP=$(ls -t "$DATA_DIR/$container"/jemalloc*.heap 2>/dev/null | head -1)
        if [ -n "$HEAP_DUMP" ]; then
            HEAP_BASENAME=$(basename "$HEAP_DUMP")
            echo "  Found heap dump: $HEAP_BASENAME"
            
            # Validate heap dump file size
            HEAP_SIZE=$(stat -f%z "$HEAP_DUMP" 2>/dev/null || stat -c%s "$HEAP_DUMP" 2>/dev/null || echo "0")
            if [ "$HEAP_SIZE" -lt 100 ]; then
                echo "  WARNING: Heap dump is very small ($HEAP_SIZE bytes), may be invalid"
            else
                echo "  Heap dump size: $HEAP_SIZE bytes"
            fi
            
            # Try to find a baseline (first heap dump) for differential analysis
            BASELINE_HEAP=$(ls -t "$DATA_DIR/$container"/jemalloc*.heap 2>/dev/null | tail -1)
            if [ -n "$BASELINE_HEAP" ] && [ "$BASELINE_HEAP" != "$HEAP_DUMP" ]; then
                BASELINE_BASENAME=$(basename "$BASELINE_HEAP")
                echo "  Using baseline: $BASELINE_BASENAME for differential analysis"
                
                # Retry logic for container operations (max 2 retries)
                RETRY_COUNT=0
                MAX_RETRIES=2
                while [ $RETRY_COUNT -le $MAX_RETRIES ]; do
                    if docker exec "$container" /profiling/scripts/generate-memory-flamegraph.sh \
                        --input "/profiling/data/$HEAP_BASENAME" \
                        --base "/profiling/data/$BASELINE_BASENAME" \
                        --output /profiling/reports/memory-flamegraph.svg \
                        --title "Memory Flamegraph (Diff) - $container" \
                        --colors mem 2>/dev/null; then
                        break
                    fi
                    
                    RETRY_COUNT=$((RETRY_COUNT + 1))
                    if [ $RETRY_COUNT -le $MAX_RETRIES ]; then
                        echo "  Retry $RETRY_COUNT/$MAX_RETRIES: Generating differential memory flamegraph..."
                        sleep 2
                    else
                        echo "  Could not generate differential memory flamegraph after $MAX_RETRIES retries"
                        # Fallback to single heap dump
                        docker exec "$container" /profiling/scripts/generate-memory-flamegraph.sh \
                            --input "/profiling/data/$HEAP_BASENAME" \
                            --output /profiling/reports/memory-flamegraph.svg \
                            --title "Memory Flamegraph - $container" \
                            --colors mem 2>/dev/null || echo "  Could not generate memory flamegraph"
                    fi
                done
            else
                # Single heap dump analysis with retry
                RETRY_COUNT=0
                MAX_RETRIES=2
                while [ $RETRY_COUNT -le $MAX_RETRIES ]; do
                    if docker exec "$container" /profiling/scripts/generate-memory-flamegraph.sh \
                        --input "/profiling/data/$HEAP_BASENAME" \
                        --output /profiling/reports/memory-flamegraph.svg \
                        --title "Memory Flamegraph - $container" \
                        --colors mem 2>/dev/null; then
                        break
                    fi
                    
                    RETRY_COUNT=$((RETRY_COUNT + 1))
                    if [ $RETRY_COUNT -le $MAX_RETRIES ]; then
                        echo "  Retry $RETRY_COUNT/$MAX_RETRIES: Generating memory flamegraph..."
                        sleep 2
                    else
                        echo "  Could not generate memory flamegraph after $MAX_RETRIES retries"
                    fi
                done
            fi
            
            # Copy generated memory flamegraphs with retry
            RETRY_COUNT=0
            MAX_RETRIES=2
            while [ $RETRY_COUNT -le $MAX_RETRIES ]; do
                if docker cp "$container:/profiling/reports/memory-flamegraph.svg" "$REPORTS_DIR/$container/" 2>/dev/null; then
                    echo "  Memory flamegraph saved"
                    break
                fi
                RETRY_COUNT=$((RETRY_COUNT + 1))
                if [ $RETRY_COUNT -le $MAX_RETRIES ]; then
                    echo "  Retry $RETRY_COUNT/$MAX_RETRIES: Copying memory flamegraph..."
                    sleep 1
                else
                    echo "  WARNING: Could not copy memory flamegraph after $MAX_RETRIES retries"
                fi
            done
            
            # Copy icicle graph (non-critical, no retry)
            docker cp "$container:/profiling/reports/memory-flamegraph-icicle.svg" "$REPORTS_DIR/$container/" 2>/dev/null || true
        else
            echo "  No heap dump files found"
        fi
    fi
done

echo ""
echo "Profiling data collection complete for $TEST_NAME"

