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

install_kernel_tools() {
    local kernel_version=$(uname -r)
    echo "[Profiling] Detected kernel: $kernel_version"
    
    if perf record -o /dev/null -- true 2>/dev/null; then
        echo "[Profiling] perf is compatible with current kernel"
        return 0
    fi
    
    echo "[Profiling] Installing kernel tools..."
    apt-get update -qq 2>/dev/null || true
    
    if apt-get install -y -qq "linux-tools-${kernel_version}" 2>/dev/null; then
        if perf record -o /dev/null -- true 2>/dev/null; then
            echo "[Profiling] perf is now working"
            return 0
        fi
    fi
    
    echo "[Profiling] WARNING: Could not install compatible kernel tools"
    echo "[Profiling] CPU profiling (flamegraphs) will not be available"
    return 1
}

start_profiling() {
    local pid=$1
    local node_name="${NODE_NAME:-merod}"
    
    echo "[Profiling] Starting profiling for PID $pid (node: $node_name)"
    
    if [ "$ENABLE_PERF" != "true" ]; then
        return
    fi
    
    if ! perf record -o /dev/null -- true 2>/dev/null; then
        echo "[Profiling] perf not compatible, skipping CPU profiling"
        return
    fi
    
    if ! kill -0 "$pid" 2>/dev/null; then
        echo "[Profiling] Process $pid is not running, cannot start perf"
        return
    fi
    
    local perf_output="$PROFILING_OUTPUT_DIR/perf-${node_name}.data"
    local perf_log="$PROFILING_OUTPUT_DIR/perf-${node_name}.log"
    
    echo "[Profiling] Starting perf record (freq: $PERF_SAMPLE_FREQ Hz)..."
    perf record -F "$PERF_SAMPLE_FREQ" -g -p "$pid" -o "$perf_output" > "$perf_log" 2>&1 &
    PERF_PID=$!
    echo $PERF_PID > "$PROFILING_OUTPUT_DIR/perf.pid"
    
    # Wait a moment to verify perf started successfully
    sleep 2
    if kill -0 "$PERF_PID" 2>/dev/null; then
        echo "[Profiling] perf recording with PID $PERF_PID"
    else
        echo "[Profiling] WARNING: perf failed to start"
        if [ -f "$perf_log" ]; then
            echo "[Profiling] perf error: $(head -3 "$perf_log" | tr '\n' ' ')"
        fi
        rm -f "$PROFILING_OUTPUT_DIR/perf.pid"
    fi
}

stop_profiling() {
    echo "[Profiling] Stopping profiling..."
    
    if [ -f "$PROFILING_OUTPUT_DIR/perf.pid" ]; then
        local perf_pid=$(cat "$PROFILING_OUTPUT_DIR/perf.pid")
        if kill -0 "$perf_pid" 2>/dev/null; then
            kill -INT "$perf_pid" 2>/dev/null || true
            
            # Wait for perf to finish writing data (up to 5 seconds)
            local wait_count=0
            while kill -0 "$perf_pid" 2>/dev/null && [ $wait_count -lt 5 ]; do
                sleep 1
                wait_count=$((wait_count + 1))
            done
            
            # If still running, force kill
            if kill -0 "$perf_pid" 2>/dev/null; then
                echo "[Profiling] WARNING: perf did not stop gracefully, forcing kill"
                kill -KILL "$perf_pid" 2>/dev/null || true
            fi
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
        exit 143
    fi
}

trap cleanup SIGTERM SIGINT

detect_jemalloc_path() {
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

if [ "$ENABLE_PERF" = "true" ]; then
    install_kernel_tools
fi

if [ "$ENABLE_JEMALLOC" = "true" ]; then
    JEMALLOC_PATH="${LD_PRELOAD_JEMALLOC:-$(detect_jemalloc_path)}"
    if [ -n "$JEMALLOC_PATH" ] && [ -f "$JEMALLOC_PATH" ]; then
        export LD_PRELOAD="$JEMALLOC_PATH"
        echo "[Profiling] jemalloc enabled (LD_PRELOAD=$LD_PRELOAD)"
        if [[ "$JEMALLOC_PATH" == "/usr/local/lib/"* ]]; then
            echo "[Profiling] Using source-built jemalloc with profiling support"
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

SHOULD_PROFILE_WITH_PERF=true
for arg in "$@"; do
    if [[ "$arg" == "init" ]] || [[ "$arg" == "--help" ]] || [[ "$arg" == "-h" ]]; then
        SHOULD_PROFILE_WITH_PERF=false
        echo "[Profiling] Skipping perf profiling for short-lived command: $arg"
        break
    fi
done

if [ "$ENABLE_PROFILING" = "true" ] && [ "$ENABLE_PERF" = "true" ] && [ "$SHOULD_PROFILE_WITH_PERF" = "true" ]; then
    "$@" &
    MAIN_PID=$!
    echo "[Profiling] Process started with PID $MAIN_PID"
    
    sleep 3
    
    if ! kill -0 "$MAIN_PID" 2>/dev/null; then
        echo "[Profiling] Process already exited, skipping perf profiling"
        wait $MAIN_PID
        EXIT_CODE=$?
        exit $EXIT_CODE
    fi
    
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
