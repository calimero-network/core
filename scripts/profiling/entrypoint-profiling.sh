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

# Global variables for signal handling
MEROD_PID=""
EXIT_CODE=0

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
    local signal_exit_code=$?
    echo "[Profiling] Received signal, cleaning up..."
    
    # Stop profiling first
    stop_profiling
    
    # Gracefully terminate merod if running
    if [ -n "$MEROD_PID" ] && kill -0 "$MEROD_PID" 2>/dev/null; then
        echo "[Profiling] Stopping merod (PID: $MEROD_PID)..."
        kill -TERM "$MEROD_PID" 2>/dev/null || true
        
        # Wait up to 10 seconds for graceful shutdown
        local wait_count=0
        while kill -0 "$MEROD_PID" 2>/dev/null && [ $wait_count -lt 10 ]; do
            sleep 1
            wait_count=$((wait_count + 1))
        done
        
        # Force kill if still running
        if kill -0 "$MEROD_PID" 2>/dev/null; then
            echo "[Profiling] Force killing merod..."
            kill -KILL "$MEROD_PID" 2>/dev/null || true
        fi
        
        echo "[Profiling] merod stopped"
    fi
    
    # Exit with appropriate code:
    # - If we have a captured EXIT_CODE from merod, use it
    # - If signal interrupted us before merod finished, use 128 + signal number
    # - Default to the signal exit code
    if [ "$EXIT_CODE" -ne 0 ]; then
        exit $EXIT_CODE
    elif [ "$signal_exit_code" -ne 0 ]; then
        exit $signal_exit_code
    else
        # SIGTERM = 15, SIGINT = 2; standard convention is 128 + signal
        exit 143  # 128 + 15 (SIGTERM)
    fi
}

trap cleanup SIGTERM SIGINT

# Build the command to run
CMD="merod"

# Auto-detect jemalloc library path based on architecture
detect_jemalloc_path() {
    if [ -f "/usr/local/lib/libjemalloc.so.2" ]; then
        echo "/usr/local/lib/libjemalloc.so.2"
        return
    fi
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
        echo "[Profiling] jemalloc enabled (LD_PRELOAD=$LD_PRELOAD)"
        if [[ "$JEMALLOC_PATH" == "/usr/local/lib/"* ]]; then
            echo "[Profiling] Using source-built jemalloc with profiling support"
        else
            echo "[Profiling] WARNING: Using system jemalloc which may lack profiling support"
            echo "[Profiling] MALLOC_CONF settings may produce 'Invalid conf pair' errors"
        fi
    else
        echo "[Profiling] jemalloc library not found, skipping jemalloc profiling"
    fi
fi

# Track if we're using heaptrack wrapper
USING_HEAPTRACK="false"

# If heaptrack is enabled, wrap the command
if [ "$ENABLE_HEAPTRACK" = "true" ]; then
    HEAPTRACK_OUTPUT="$PROFILING_OUTPUT_DIR/heaptrack-${NODE_NAME:-merod}"
    CMD="heaptrack -o $HEAPTRACK_OUTPUT $CMD"
    USING_HEAPTRACK="true"
    echo "[Profiling] heaptrack enabled (output: $HEAPTRACK_OUTPUT)"
    if [ "$ENABLE_PERF" = "true" ]; then
        echo "[Profiling] WARNING: Both heaptrack and perf enabled. Will attempt to find actual merod PID for perf."
    fi
fi

# Start the main process
echo "[Profiling] Starting: $CMD $@"

if [ "$ENABLE_PROFILING" = "true" ] && [ "$ENABLE_PERF" = "true" ]; then
    # Start merod (or heaptrack wrapper) in background
    $CMD "$@" &
    WRAPPER_PID=$!
    MEROD_PID=$WRAPPER_PID
    echo "[Profiling] Process started with PID $WRAPPER_PID"
    
    # Give the process time to initialize
    sleep 2
    
    # If heaptrack is wrapping merod, find the actual merod child process
    if [ "$USING_HEAPTRACK" = "true" ]; then
        # Find merod child process spawned by heaptrack
        ACTUAL_MEROD_PID=$(pgrep -P "$WRAPPER_PID" -x merod 2>/dev/null | head -1 || true)
        if [ -n "$ACTUAL_MEROD_PID" ]; then
            echo "[Profiling] Found actual merod child process: PID $ACTUAL_MEROD_PID (parent heaptrack: $WRAPPER_PID)"
            PERF_TARGET_PID=$ACTUAL_MEROD_PID
        else
            echo "[Profiling] WARNING: Could not find merod child process, attaching perf to heaptrack wrapper"
            PERF_TARGET_PID=$WRAPPER_PID
        fi
    else
        PERF_TARGET_PID=$MEROD_PID
    fi
    
    # Start profiling on the correct target
    start_profiling $PERF_TARGET_PID
    
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

