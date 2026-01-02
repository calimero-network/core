#!/bin/bash
# Profiling-enabled entrypoint for merod

set -e

ENABLE_PROFILING="${ENABLE_PROFILING:-true}"
ENABLE_JEMALLOC="${ENABLE_JEMALLOC:-true}"
ENABLE_PERF="${ENABLE_PERF:-true}"
ENABLE_HEAPTRACK="${ENABLE_HEAPTRACK:-false}"
ENABLE_WASMER_PROFILING="${ENABLE_WASMER_PROFILING:-true}"
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
    
    sleep 2
    if ! kill -0 "$PERF_PID" 2>/dev/null; then
        echo "[Profiling] ERROR: perf process died immediately"
        if [ -f "$perf_log" ]; then
            echo "[Profiling] perf error log:"
            cat "$perf_log" | head -20
        fi
        rm -f "$PROFILING_OUTPUT_DIR/perf.pid"
        return
    fi
    
    echo "[Profiling] perf recording with PID $PERF_PID"
    
    sleep 2
    
    # Check if perf is still running
    if ! kill -0 "$PERF_PID" 2>/dev/null; then
        echo "[Profiling] ERROR: perf process died"
        if [ -f "$perf_log" ]; then
            echo "[Profiling] perf error log:"
            cat "$perf_log" | head -20
        fi
        rm -f "$PROFILING_OUTPUT_DIR/perf.pid"
        return
    fi
    
    # Check process CPU usage for informational purposes
    if command -v ps >/dev/null 2>&1; then
        local cpu_usage=$(ps -p "$pid" -o %cpu= 2>/dev/null | tr -d ' ' || echo "N/A")
        echo "[Profiling] Target process CPU usage: ${cpu_usage}%"
        
        if [ "$cpu_usage" != "N/A" ] && [ -n "$cpu_usage" ]; then
            local cpu_int=$(echo "$cpu_usage" | awk -F. '{print $1}')
            if [ -n "$cpu_int" ] && [ "$cpu_int" -lt 5 ] 2>/dev/null; then
                echo "[Profiling] Note: Low CPU usage (${cpu_usage}%) may result in fewer samples. perf buffers data and writes periodically."
            fi
        fi
    fi
    
    echo "[Profiling] ✓ perf is running. Data will be collected and written periodically."
    
    # Monitor for perf.map file generation (for WASM JIT code symbolization)
    if [ "$ENABLE_WASMER_PROFILING" = "true" ]; then
        if [ -z "$pid" ]; then
            echo "[Profiling] WARNING: PID not available, cannot monitor perf.map file"
        else
            (
                echo "[Profiling] Monitoring for perf.map file generation..."
                check_count=0
                max_checks=30
                while [ $check_count -lt $max_checks ]; do
                    sleep 2
                    check_count=$((check_count + 1))
                    perf_map="/tmp/perf-${pid}.map"
                    if [ -f "$perf_map" ]; then
                        map_size=$(stat -f%z "$perf_map" 2>/dev/null || stat -c%s "$perf_map" 2>/dev/null || echo "0")
                        echo "[Profiling] ✓ perf.map file detected: $perf_map ($map_size bytes)"
                        echo "[Profiling]   This file enables WASM function name symbolization in flamegraphs"
                        break
                    fi
                done
                if [ $check_count -eq $max_checks ]; then
                    echo "[Profiling] Note: perf.map file not detected after ${max_checks} checks (60 seconds)"
                fi
            ) &
        fi
    fi
}

stop_profiling() {
    echo "[Profiling] Stopping profiling..."
    
    if [ -f "$PROFILING_OUTPUT_DIR/perf.pid" ]; then
        local perf_pid=$(cat "$PROFILING_OUTPUT_DIR/perf.pid")
        if kill -0 "$perf_pid" 2>/dev/null; then
            kill -INT "$perf_pid" 2>/dev/null || true
            
            local wait_count=0
            while kill -0 "$perf_pid" 2>/dev/null && [ $wait_count -lt 5 ]; do
                sleep 1
                wait_count=$((wait_count + 1))
            done
            
            if kill -0 "$perf_pid" 2>/dev/null; then
                echo "[Profiling] WARNING: perf did not stop gracefully, forcing kill"
                kill -KILL "$perf_pid" 2>/dev/null || true
            fi
        fi
        rm -f "$PROFILING_OUTPUT_DIR/perf.pid"
        
        local perf_files=$(ls "$PROFILING_OUTPUT_DIR"/perf-*.data 2>/dev/null || true)
        if [ -n "$perf_files" ]; then
            for perf_file in $perf_files; do
                local file_size=$(stat -f%z "$perf_file" 2>/dev/null || stat -c%s "$perf_file" 2>/dev/null || echo "0")
                
                if [ "$file_size" -lt 1000 ]; then
                    echo "[Profiling] WARNING: perf data file is very small: $perf_file ($file_size bytes)"
                    echo "[Profiling]   perf may have collected minimal/no samples"
                else
                    local sample_count=$(perf report -i "$perf_file" --stdio 2>/dev/null | grep -E "^# Samples:" | head -1 | awk '{print $3}' || echo "unknown")
                    
                    if [ "$sample_count" != "unknown" ] && [ "$sample_count" != "0" ]; then
                        echo "[Profiling] ✓ perf data file created: $perf_file ($file_size bytes, $sample_count samples)"
                    else
                        echo "[Profiling] perf data file exists but may be empty: $perf_file ($file_size bytes, samples: $sample_count)"
                    fi
                fi
            done
        else
            echo "[Profiling] WARNING: No perf data files found in $PROFILING_OUTPUT_DIR"
        fi
    fi
    
    # Preserve perf.map files for JIT code symbolization
    # Wasmer writes perf.map files to /tmp/perf-<pid>.map for WASM function names
    if [ "$ENABLE_WASMER_PROFILING" = "true" ]; then
        local merod_pid=$(pgrep -x merod 2>/dev/null | head -1)
        if [ -n "$merod_pid" ]; then
            local perf_map="/tmp/perf-${merod_pid}.map"
            if [ -f "$perf_map" ]; then
                local perf_map_copy="$PROFILING_OUTPUT_DIR/perf-${NODE_NAME:-merod}-${merod_pid}.map"
                echo "[Profiling] Copying perf.map file for WASM symbolization..."
                cp "$perf_map" "$perf_map_copy" 2>/dev/null || true
                if [ -f "$perf_map_copy" ]; then
                    local map_size=$(stat -f%z "$perf_map_copy" 2>/dev/null || stat -c%s "$perf_map_copy" 2>/dev/null || echo "0")
                    echo "[Profiling] ✓ perf.map file preserved: $(basename "$perf_map_copy") ($map_size bytes)"
                else
                    echo "[Profiling] WARNING: Could not copy perf.map file"
                fi
            else
                echo "[Profiling] Note: No perf.map file found at $perf_map (WASM profiling may not be active)"
            fi
        fi
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