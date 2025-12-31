#!/bin/bash
# Stop profiling and collect data
# Usage: stop-profiling.sh [OPTIONS]
#
# Options:
#   --node-name NAME    Name of the node (for finding PID files)
#   --output-dir DIR    Output directory for profiling data
#   --generate-reports  Generate reports after stopping (flamegraphs, etc.)

set -e

# Default values
NODE_NAME="merod"
OUTPUT_DIR="${PROFILING_OUTPUT_DIR:-/profiling/data}"
GENERATE_REPORTS=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --node-name)
            NODE_NAME="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --generate-reports)
            GENERATE_REPORTS=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo "Stopping profiling for $NODE_NAME..."

# Stop perf
PERF_PID_FILE="$OUTPUT_DIR/perf-${NODE_NAME}.pid"
if [ -f "$PERF_PID_FILE" ]; then
    PERF_PID=$(cat "$PERF_PID_FILE")
    if kill -0 "$PERF_PID" 2>/dev/null; then
        echo "Stopping perf (PID: $PERF_PID)..."
        kill -INT "$PERF_PID" 2>/dev/null || true
        # Wait for perf to finish writing data
        sleep 3
        # Force kill if still running
        if kill -0 "$PERF_PID" 2>/dev/null; then
            kill -KILL "$PERF_PID" 2>/dev/null || true
        fi
    fi
    rm -f "$PERF_PID_FILE"
    echo "perf stopped"
fi

# Stop memory stats collection
MEMSTATS_PID_FILE="$OUTPUT_DIR/memstats-${NODE_NAME}.pid"
if [ -f "$MEMSTATS_PID_FILE" ]; then
    MEMSTATS_PID=$(cat "$MEMSTATS_PID_FILE")
    if kill -0 "$MEMSTATS_PID" 2>/dev/null; then
        echo "Stopping memory stats collection (PID: $MEMSTATS_PID)..."
        kill -TERM "$MEMSTATS_PID" 2>/dev/null || true
    fi
    rm -f "$MEMSTATS_PID_FILE"
    echo "Memory stats collection stopped"
fi

# Trigger final jemalloc heap dump if process is still running
MEROD_PID=$(pgrep -x merod 2>/dev/null | head -1)
if [ -n "$MEROD_PID" ]; then
    echo "Triggering final jemalloc heap dump..."
    kill -USR1 "$MEROD_PID" 2>/dev/null || true
    sleep 1
    
    # Preserve perf.map files for JIT code symbolization (if Wasmer profiling is enabled)
    if [ "${ENABLE_WASMER_PROFILING:-true}" = "true" ]; then
        PERF_MAP="/tmp/perf-${MEROD_PID}.map"
        if [ -f "$PERF_MAP" ]; then
            PERF_MAP_COPY="$OUTPUT_DIR/perf-${NODE_NAME}-${MEROD_PID}.map"
            echo "Copying perf.map file for WASM symbolization..."
            cp "$PERF_MAP" "$PERF_MAP_COPY" 2>/dev/null || true
            if [ -f "$PERF_MAP_COPY" ]; then
                MAP_SIZE=$(stat -f%z "$PERF_MAP_COPY" 2>/dev/null || stat -c%s "$PERF_MAP_COPY" 2>/dev/null || echo "0")
                echo "  ✓ perf.map file preserved: $(basename "$PERF_MAP_COPY") ($MAP_SIZE bytes)"
            else
                echo "  WARNING: Could not copy perf.map file"
            fi
        fi
    fi
fi

echo ""
echo "Profiling stopped for $NODE_NAME"
echo "Data files in: $OUTPUT_DIR"
ls -la "$OUTPUT_DIR"/ 2>/dev/null || true

# Generate reports if requested
if [ "$GENERATE_REPORTS" = true ]; then
    REPORTS_DIR="${PROFILING_REPORTS_DIR:-/profiling/reports}"
    echo ""
    echo "Generating reports..."
    
    # Generate flamegraph if perf data exists
    PERF_DATA=$(ls -t "$OUTPUT_DIR"/perf-${NODE_NAME}*.data 2>/dev/null | head -1)
    if [ -n "$PERF_DATA" ] && [ -f "$PERF_DATA" ]; then
        echo "Generating flamegraph from $PERF_DATA..."
        /profiling/scripts/generate-flamegraph.sh \
            --input "$PERF_DATA" \
            --output "$REPORTS_DIR/flamegraph-${NODE_NAME}.svg" \
            --title "CPU Flamegraph - $NODE_NAME"
    fi
    
    # Generate memory report
    echo "Generating memory report..."
    /profiling/scripts/generate-memory-report.sh \
        --node-name "$NODE_NAME" \
        --input-dir "$OUTPUT_DIR" \
        --output "$REPORTS_DIR/memory-report-${NODE_NAME}.txt"
    
    # Generate memory flamegraph
    echo "Generating memory flamegraph..."
    HEAP_DUMP=$(ls -t "$OUTPUT_DIR"/jemalloc*.heap 2>/dev/null | head -1)
    if [ -n "$HEAP_DUMP" ]; then
        # Try to find a baseline for differential analysis
        BASELINE_HEAP=$(ls -t "$OUTPUT_DIR"/jemalloc*.heap 2>/dev/null | tail -1)
        if [ -n "$BASELINE_HEAP" ] && [ "$BASELINE_HEAP" != "$HEAP_DUMP" ]; then
            /profiling/scripts/generate-memory-flamegraph.sh \
                --input "$HEAP_DUMP" \
                --base "$BASELINE_HEAP" \
                --output "$REPORTS_DIR/memory-flamegraph-${NODE_NAME}.svg" \
                --title "Memory Flamegraph (Diff) - $NODE_NAME" \
                --colors mem || echo "Warning: Memory flamegraph generation failed"
        else
            /profiling/scripts/generate-memory-flamegraph.sh \
                --input "$HEAP_DUMP" \
                --output "$REPORTS_DIR/memory-flamegraph-${NODE_NAME}.svg" \
                --title "Memory Flamegraph - $NODE_NAME" \
                --colors mem || echo "Warning: Memory flamegraph generation failed"
        fi
    fi
    
    echo ""
    echo "Reports generated in: $REPORTS_DIR"
    ls -la "$REPORTS_DIR"/ 2>/dev/null || true
fi

