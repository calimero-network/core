#!/bin/bash
# Profiling-enabled entrypoint for merod

set -e

ENABLE_PROFILING="${ENABLE_PROFILING:-true}"
ENABLE_JEMALLOC="${ENABLE_JEMALLOC:-true}"
ENABLE_PERF="${ENABLE_PERF:-true}"
ENABLE_HEAPTRACK="${ENABLE_HEAPTRACK:-false}"
PROFILING_OUTPUT_DIR="${PROFILING_OUTPUT_DIR:-/profiling/data}"
PERF_SAMPLE_FREQ="${PERF_SAMPLE_FREQ:-99}"

MAIN_PID=""
EXIT_CODE=0

mkdir -p "$PROFILING_OUTPUT_DIR"
mkdir -p "${PROFILING_REPORTS_DIR:-/profiling/reports}"

try_install_kernel_tools() {
    local kernel_version=$(uname -r)
    echo "[Profiling] Detected kernel: $kernel_version"
    
    # Check if perf works with current kernel
    if perf --version >/dev/null 2>&1 && perf record -o /dev/null -- true 2>/dev/null; then
        echo "[Profiling] perf is compatible with current kernel"
        return 0
    fi
    
    echo "[Profiling] perf not compatible, attempting to install matching kernel tools..."
    
    # Try to install matching kernel tools (requires network)
    if [[ "$kernel_version" == *"-azure"* ]]; then
        echo "[Profiling] Azure kernel detected"
        # Try metapackage first (pulls in correct version automatically)
        if apt-get update -qq 2>/dev/null && apt-get install -y -qq linux-tools-azure 2>/dev/null; then
            echo "[Profiling] Installed linux-tools-azure"
            return 0
        fi
        # Fallback to specific version
        if apt-get install -y -qq "linux-tools-${kernel_version}" 2>/dev/null; then
            echo "[Profiling] Installed linux-tools-${kernel_version}"
            return 0
        fi
    elif [[ "$kernel_version" == *"-aws"* ]]; then
        echo "[Profiling] AWS kernel detected"
        if apt-get update -qq 2>/dev/null && apt-get install -y -qq linux-tools-aws 2>/dev/null; then
            echo "[Profiling] Installed linux-tools-aws"
            return 0
        fi
    elif [[ "$kernel_version" == *"-gcp"* ]]; then
        echo "[Profiling] GCP kernel detected"
        if apt-get update -qq 2>/dev/null && apt-get install -y -qq linux-tools-gcp 2>/dev/null; then
            echo "[Profiling] Installed linux-tools-gcp"
            return 0
        fi
    else
        # Try generic version
        if apt-get update -qq 2>/dev/null && apt-get install -y -qq "linux-tools-${kernel_version}" 2>/dev/null; then
            echo "[Profiling] Installed linux-tools-${kernel_version}"
            return 0
        fi
    fi
    
    echo "[Profiling] WARNING: Could not install kernel-specific perf tools"
    echo "[Profiling] Kernel: $kernel_version"
    echo "[Profiling] CPU profiling (flamegraphs) will not be available"
    echo "[Profiling] Memory profiling (jemalloc) still works"
    return 1
}

start_profiling() {
    local pid=$1
    local node_name="${NODE_NAME:-merod}"
    
    echo "[Profiling] Starting profiling for PID $pid (node: $node_name)"
    
    if [ "$ENABLE_PERF" = "true" ]; then
        # Check if perf is available and compatible
        if ! perf record -o /dev/null -- true 2>/dev/null; then
            echo "[Profiling] perf not compatible with host kernel, skipping CPU profiling"
            return
        fi
        
        echo "[Profiling] Starting perf record (freq: $PERF_SAMPLE_FREQ Hz)..."
        perf record -F "$PERF_SAMPLE_FREQ" -g -p "$pid" \
            -o "$PROFILING_OUTPUT_DIR/perf-${node_name}.data" 2>&1 &
        PERF_PID=$!
        echo $PERF_PID > "$PROFILING_OUTPUT_DIR/perf.pid"
        
        sleep 1
        if kill -0 "$PERF_PID" 2>/dev/null; then
            echo "[Profiling] perf recording with PID $PERF_PID"
        else
            echo "[Profiling] WARNING: perf failed to start"
            rm -f "$PROFILING_OUTPUT_DIR/perf.pid"
        fi
    fi
}

stop_profiling() {
    echo "[Profiling] Stopping profiling..."
    
    if [ -f "$PROFILING_OUTPUT_DIR/perf.pid" ]; then
        local perf_pid=$(cat "$PROFILING_OUTPUT_DIR/perf.pid")
        if kill -0 "$perf_pid" 2>/dev/null; then
            kill -INT "$perf_pid" 2>/dev/null || true
            sleep 2
        fi
        rm -f "$PROFILING_OUTPUT_DIR/perf.pid"
    fi
}

cleanup() {
    local signal_exit_code=$?
    echo "[Profiling] Received signal, cleaning up..."
    
    stop_profiling
    
    if [ -n "$MAIN_PID" ] && kill -0 "$MAIN_PID" 2>/dev/null; then
        echo "[Profiling] Stopping main process (PID: $MAIN_PID)..."
        kill -TERM "$MAIN_PID" 2>/dev/null || true
        
        local wait_count=0
        while kill -0 "$MAIN_PID" 2>/dev/null && [ $wait_count -lt 10 ]; do
            sleep 1
            wait_count=$((wait_count + 1))
        done
        
        if kill -0 "$MAIN_PID" 2>/dev/null; then
            kill -KILL "$MAIN_PID" 2>/dev/null || true
        fi
    fi
    
    if [ "$EXIT_CODE" -ne 0 ]; then
        exit $EXIT_CODE
    elif [ "$signal_exit_code" -ne 0 ]; then
        exit $signal_exit_code
    else
        exit 143  # 128 + 15 (SIGTERM)
    fi
}

trap cleanup SIGTERM SIGINT

detect_jemalloc_path() {
    # Prefer source-built jemalloc (compiled with --enable-prof)
    if [ -f "/usr/local/lib/libjemalloc.so.2" ]; then
        echo "/usr/local/lib/libjemalloc.so.2"
        return
    fi
    local arch=$(uname -m)
    case "$arch" in
        x86_64)    echo "/usr/lib/x86_64-linux-gnu/libjemalloc.so.2" ;;
        aarch64)   echo "/usr/lib/aarch64-linux-gnu/libjemalloc.so.2" ;;
        *)         echo "" ;;
    esac
}

# Try to ensure perf is compatible with host kernel
if [ "$ENABLE_PERF" = "true" ]; then
    try_install_kernel_tools
fi

if [ "$ENABLE_JEMALLOC" = "true" ]; then
    JEMALLOC_PATH="${LD_PRELOAD_JEMALLOC:-$(detect_jemalloc_path)}"
    if [ -n "$JEMALLOC_PATH" ] && [ -f "$JEMALLOC_PATH" ]; then
        export LD_PRELOAD="$JEMALLOC_PATH"
        echo "[Profiling] jemalloc enabled (LD_PRELOAD=$LD_PRELOAD)"
        if [[ "$JEMALLOC_PATH" == "/usr/local/lib/"* ]]; then
            echo "[Profiling] Using source-built jemalloc with profiling support"
        else
            echo "[Profiling] WARNING: System jemalloc may lack profiling support"
        fi
    else
        echo "[Profiling] jemalloc library not found, skipping"
    fi
fi

if [ "$ENABLE_HEAPTRACK" = "true" ]; then
    HEAPTRACK_OUTPUT="$PROFILING_OUTPUT_DIR/heaptrack-${NODE_NAME:-merod}"
    set -- heaptrack -o "$HEAPTRACK_OUTPUT" "$@"
    echo "[Profiling] heaptrack enabled (output: $HEAPTRACK_OUTPUT)"
fi

echo "[Profiling] Executing: $@"

if [ "$ENABLE_PROFILING" = "true" ] && [ "$ENABLE_PERF" = "true" ]; then
    "$@" &
    MAIN_PID=$!
    echo "[Profiling] Process started with PID $MAIN_PID"
    
    sleep 2
    
    if [ "$ENABLE_HEAPTRACK" = "true" ]; then
        ACTUAL_PID=$(pgrep -P "$MAIN_PID" 2>/dev/null | head -1 || echo "$MAIN_PID")
        if [ "$ACTUAL_PID" != "$MAIN_PID" ]; then
            echo "[Profiling] Found child process: PID $ACTUAL_PID"
        fi
        PERF_TARGET_PID=$ACTUAL_PID
    else
        PERF_TARGET_PID=$MAIN_PID
    fi
    
    start_profiling $PERF_TARGET_PID
    
    wait $MAIN_PID
    EXIT_CODE=$?
    
    stop_profiling
    
    exit $EXIT_CODE
else
    exec "$@"
fi
