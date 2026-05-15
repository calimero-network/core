#!/bin/bash
# Collect profiling data from Docker containers
# Usage: collect-from-containers.sh <test-name> <logs-dir> <data-dir> <reports-dir>
#
# NOTE: `merobox bootstrap run` tears the runtime nodes down on exit, so by the
# time this step runs the `fuzzy-*-node-N` containers are usually already gone
# and the `docker exec`/`docker cp` calls below are no-ops. Container logs are
# kept by the workflow's live `docker logs -f` watcher; the perf `.data` +
# jemalloc heaps survive via the host bind mount — `entrypoint-profiling.sh`
# mirrors `/profiling/data` to `$CALIMERO_HOME/profiling-dump` on shutdown and
# `harvest-host-profiling.sh` picks that up afterwards (that's the authoritative
# collector). This script is best-effort extra coverage for the rare case a
# container is still alive.

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
                    # Wait up to 30s for perf to flush its final buffer
                    # before SIGKILL. Loop exits as soon as perf exits, so
                    # successful runs see no additional latency.
                    for i in $(seq 1 30); do
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

        # Flamegraph rendering happens inside the image entrypoint's
        # preserve_to_host_mount; harvest-host-profiling.sh picks up the
        # SVGs from the bind mount.

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

