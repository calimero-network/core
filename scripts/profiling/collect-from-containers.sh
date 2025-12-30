#!/bin/bash
# Collect profiling data from Docker containers
# Usage: collect-from-containers.sh <test-name> <logs-dir> <data-dir> <reports-dir>

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
        
        mkdir -p "$DATA_DIR/$container"
        mkdir -p "$REPORTS_DIR/$container"
        
        # Stop perf gracefully to flush buffered samples
        echo "  Stopping perf profiler..."
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
        ' 2>/dev/null || echo "  Could not stop perf (container may have stopped)"
        
        sleep 1
        
        # Check if container is running
        CONTAINER_RUNNING=false
        if docker ps -q -f "name=$container" 2>/dev/null | grep -q .; then
            CONTAINER_RUNNING=true
            echo "  Container is running"
        else
            echo "  WARNING: Container not running - collecting existing data only"
        fi
        
        # Generate reports inside container while it's still running
        if [ "$CONTAINER_RUNNING" = "true" ]; then
            # CPU flamegraph
            echo "  Generating CPU flamegraph..."
            PERF_FILE=$(docker exec "$container" bash -c 'ls -t /profiling/data/perf-*.data 2>/dev/null | head -1' 2>/dev/null || true)
            if [ -n "$PERF_FILE" ]; then
                PERF_BASENAME=$(basename "$PERF_FILE")
                echo "    Found perf data: $PERF_BASENAME"
                docker exec "$container" /profiling/scripts/generate-flamegraph.sh \
                    --input "/profiling/data/$PERF_BASENAME" \
                    --output /profiling/reports/flamegraph.svg \
                    --title "CPU Flamegraph - $container" 2>&1 || echo "    Could not generate CPU flamegraph"
            else
                echo "    No perf data found"
            fi
            
            # Memory report
            echo "  Generating memory report..."
            docker exec "$container" /profiling/scripts/generate-memory-report.sh \
                --node-name "$container" \
                --output /profiling/reports/memory-report.txt 2>&1 || echo "    Could not generate memory report"
            
            # Memory flamegraph
            echo "  Generating memory flamegraph..."
            HEAP_FILE=$(docker exec "$container" bash -c 'ls -t /profiling/data/jemalloc*.heap 2>/dev/null | head -1' 2>/dev/null || true)
            if [ -n "$HEAP_FILE" ]; then
                HEAP_BASENAME=$(basename "$HEAP_FILE")
                echo "    Found heap dump: $HEAP_BASENAME"
                
                BASELINE_FILE=$(docker exec "$container" bash -c 'ls -t /profiling/data/jemalloc*.heap 2>/dev/null | tail -1' 2>/dev/null || true)
                BASELINE_BASENAME=""
                if [ -n "$BASELINE_FILE" ]; then
                    BASELINE_BASENAME=$(basename "$BASELINE_FILE")
                fi
                
                if [ -n "$BASELINE_BASENAME" ] && [ "$BASELINE_BASENAME" != "$HEAP_BASENAME" ]; then
                    echo "    Using baseline: $BASELINE_BASENAME for differential analysis"
                    docker exec "$container" /profiling/scripts/generate-memory-flamegraph.sh \
                        --input "/profiling/data/$HEAP_BASENAME" \
                        --base "/profiling/data/$BASELINE_BASENAME" \
                        --output /profiling/reports/memory-flamegraph.svg \
                        --title "Memory Flamegraph (Diff) - $container" \
                        --colors mem 2>&1 || {
                            echo "    Differential analysis failed, trying single heap dump..."
                            docker exec "$container" /profiling/scripts/generate-memory-flamegraph.sh \
                                --input "/profiling/data/$HEAP_BASENAME" \
                                --output /profiling/reports/memory-flamegraph.svg \
                                --title "Memory Flamegraph - $container" \
                                --colors mem 2>&1 || echo "    Could not generate memory flamegraph"
                        }
                else
                    echo "    Using single heap dump analysis"
                    docker exec "$container" /profiling/scripts/generate-memory-flamegraph.sh \
                        --input "/profiling/data/$HEAP_BASENAME" \
                        --output /profiling/reports/memory-flamegraph.svg \
                        --title "Memory Flamegraph - $container" \
                        --colors mem 2>&1 || echo "    Could not generate memory flamegraph"
                fi
            else
                echo "    No heap dumps found"
            fi
        fi
        
        # Copy data and reports from container to host
        echo "  Copying profiling data..."
        docker cp "$container:/profiling/data/." "$DATA_DIR/$container/" 2>/dev/null || echo "    No profiling data in $container"
        docker cp "$container:/profiling/reports/." "$REPORTS_DIR/$container/" 2>/dev/null || echo "    No profiling reports in $container"
        
        # Collect container logs
        docker logs "$container" > "$LOGS_DIR/${container}.log" 2>&1 || true
        echo "  Collected container logs"
    fi
done

echo ""
echo "Profiling data collection complete for $TEST_NAME"

