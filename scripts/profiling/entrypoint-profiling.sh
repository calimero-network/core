#!/bin/bash
# Profiling-enabled entrypoint for merod
# This script wraps merod with optional profiling capabilities
#
# This image enables profiling by default since users explicitly choose
# the profiling image (merod:edge-profiling) when they want profiling.
# Set ENABLE_PROFILING=false to disable.

set -e

# Configuration from environment
# Profiling is ENABLED by default in the profiling image
ENABLE_PROFILING="${ENABLE_PROFILING:-true}"
ENABLE_JEMALLOC="${ENABLE_JEMALLOC:-true}"
ENABLE_PERF="${ENABLE_PERF:-true}"
ENABLE_HEAPTRACK="${ENABLE_HEAPTRACK:-false}"
PROFILING_OUTPUT_DIR="${PROFILING_OUTPUT_DIR:-/profiling/data}"
PERF_SAMPLE_FREQ="${PERF_SAMPLE_FREQ:-99}"

# Ensure profiling directories exist
mkdir -p "$PROFILING_OUTPUT_DIR"
mkdir -p "${PROFILING_REPORTS_DIR:-/profiling/reports}"

# Function to start profiling
start_profiling() {
    local pid=$1
    local node_name="${NODE_NAME:-merod}"
    
    echo "[Profiling] Starting profiling for PID $pid (node: $node_name)"
    
    if [ "$ENABLE_PERF" = "true" ]; then
        echo "[Profiling] Starting perf record..."
        # Start perf in background, recording to a file
        perf record -F "$PERF_SAMPLE_FREQ" -g -p "$pid" \
            -o "$PROFILING_OUTPUT_DIR/perf-${node_name}.data" &
        echo $! > "$PROFILING_OUTPUT_DIR/perf.pid"
        echo "[Profiling] perf started with PID $(cat $PROFILING_OUTPUT_DIR/perf.pid)"
    fi
}

# Function to stop profiling and collect data
stop_profiling() {
    echo "[Profiling] Stopping profiling..."
    
    # Stop perf if running
    if [ -f "$PROFILING_OUTPUT_DIR/perf.pid" ]; then
        local perf_pid=$(cat "$PROFILING_OUTPUT_DIR/perf.pid")
        if kill -0 "$perf_pid" 2>/dev/null; then
            echo "[Profiling] Stopping perf (PID: $perf_pid)..."
            kill -INT "$perf_pid" 2>/dev/null || true
            sleep 2
        fi
        rm -f "$PROFILING_OUTPUT_DIR/perf.pid"
    fi
    
    echo "[Profiling] Profiling stopped"
}

# Trap signals to ensure cleanup
cleanup() {
    echo "[Profiling] Received signal, cleaning up..."
    stop_profiling
    exit 0
}

trap cleanup SIGTERM SIGINT

# Build the command to run
CMD="merod"

# Auto-detect jemalloc library path based on architecture
detect_jemalloc_path() {
    local arch=$(uname -m)
    case "$arch" in
        x86_64)
            echo "/usr/lib/x86_64-linux-gnu/libjemalloc.so.2"
            ;;
        aarch64|arm64)
            echo "/usr/lib/aarch64-linux-gnu/libjemalloc.so.2"
            ;;
        *)
            echo ""
            ;;
    esac
}

# If jemalloc profiling is enabled, preload the library
if [ "$ENABLE_JEMALLOC" = "true" ]; then
    # Use provided path or auto-detect
    JEMALLOC_PATH="${LD_PRELOAD_JEMALLOC:-$(detect_jemalloc_path)}"
    if [ -n "$JEMALLOC_PATH" ] && [ -f "$JEMALLOC_PATH" ]; then
        export LD_PRELOAD="$JEMALLOC_PATH"
        echo "[Profiling] jemalloc profiling enabled (LD_PRELOAD=$LD_PRELOAD)"
    else
        echo "[Profiling] jemalloc library not found, skipping jemalloc profiling"
    fi
fi

# If heaptrack is enabled, wrap the command
if [ "$ENABLE_HEAPTRACK" = "true" ]; then
    HEAPTRACK_OUTPUT="$PROFILING_OUTPUT_DIR/heaptrack-${NODE_NAME:-merod}"
    CMD="heaptrack -o $HEAPTRACK_OUTPUT $CMD"
    echo "[Profiling] heaptrack enabled (output: $HEAPTRACK_OUTPUT)"
fi

# Start the main process
echo "[Profiling] Starting: $CMD $@"

if [ "$ENABLE_PROFILING" = "true" ] && [ "$ENABLE_PERF" = "true" ]; then
    # Start merod in background, then attach perf
    $CMD "$@" &
    MEROD_PID=$!
    echo "[Profiling] merod started with PID $MEROD_PID"
    
    # Give the process time to initialize
    sleep 2
    
    # Start profiling
    start_profiling $MEROD_PID
    
    # Wait for merod to exit
    wait $MEROD_PID
    EXIT_CODE=$?
    
    # Stop profiling
    stop_profiling
    
    exit $EXIT_CODE
else
    # Run directly without perf attachment
    exec $CMD "$@"
fi

