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
        
        # Flamegraph generation used to happen here via `docker exec` against
        # a still-running container. But the runtime merod containers are
        # removed during merobox graceful shutdown, BEFORE this collector
        # runs, so the "docker exec" path was a silent no-op — it only ever
        # reached stopped init containers, which don't have a merod process
        # or perf.data. Flamegraph rendering now happens inside the
        # profiling-image entrypoint's preserve_to_host_mount (see
        # scripts/profiling/entrypoint-profiling.sh), with the SVGs landing
        # under $CALIMERO_HOME/profiling-dump/reports/ and picked up by
        # harvest-host-profiling.sh on the host.


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

