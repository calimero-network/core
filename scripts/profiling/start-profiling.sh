#!/bin/bash
# Start profiling for a merod process
# Usage: start-profiling.sh [OPTIONS]
#
# Options:
#   --pid PID           Process ID to profile (required if not auto-detecting)
#   --node-name NAME    Name of the node (for output file naming)
#   --output-dir DIR    Output directory for profiling data
#   --perf              Enable perf profiling
#   --jemalloc          Enable jemalloc profiling
#   --heaptrack         Enable heaptrack profiling
#   --sample-freq FREQ  Perf sample frequency (default: 99)
#   --duration SEC      Duration to profile in seconds (0 = until stopped)

set -e

# Default values
PID=""
NODE_NAME="merod"
OUTPUT_DIR="${PROFILING_OUTPUT_DIR:-/profiling/data}"
ENABLE_PERF=false
ENABLE_JEMALLOC=false
ENABLE_HEAPTRACK=false
SAMPLE_FREQ="${PERF_SAMPLE_FREQ:-99}"
DURATION=0

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --pid)
            PID="$2"
            shift 2
            ;;
        --node-name)
            NODE_NAME="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --perf)
            ENABLE_PERF=true
            shift
            ;;
        --jemalloc)
            ENABLE_JEMALLOC=true
            shift
            ;;
        --heaptrack)
            ENABLE_HEAPTRACK=true
            shift
            ;;
        --sample-freq)
            SAMPLE_FREQ="$2"
            shift 2
            ;;
        --duration)
            DURATION="$2"
            shift 2
            ;;
        --all)
            ENABLE_PERF=true
            ENABLE_JEMALLOC=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Auto-detect merod PID if not specified
if [ -z "$PID" ]; then
    PID=$(pgrep -x merod 2>/dev/null | head -1)
    if [ -z "$PID" ]; then
        echo "Error: Could not find merod process. Specify --pid manually."
        exit 1
    fi
    echo "Auto-detected merod PID: $PID"
fi

# Verify process exists
if ! kill -0 "$PID" 2>/dev/null; then
    echo "Error: Process $PID does not exist"
    exit 1
fi

# Create output directory
mkdir -p "$OUTPUT_DIR"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
echo "Starting profiling for $NODE_NAME (PID: $PID) at $TIMESTAMP"

# Start perf profiling
if [ "$ENABLE_PERF" = true ]; then
    PERF_OUTPUT="$OUTPUT_DIR/perf-${NODE_NAME}-${TIMESTAMP}.data"
    echo "Starting perf record (output: $PERF_OUTPUT, freq: $SAMPLE_FREQ Hz)..."
    
    PERF_CMD="perf record -F $SAMPLE_FREQ -g -p $PID -o $PERF_OUTPUT"
    
    if [ "$DURATION" -gt 0 ]; then
        # Run for specified duration
        timeout "${DURATION}s" $PERF_CMD &
    else
        # Run until stopped
        $PERF_CMD &
    fi
    
    PERF_PID=$!
    echo "$PERF_PID" > "$OUTPUT_DIR/perf-${NODE_NAME}.pid"
    echo "perf started with PID $PERF_PID"
fi

# Configure jemalloc profiling via signals
if [ "$ENABLE_JEMALLOC" = true ]; then
    echo "Triggering jemalloc heap dump..."
    # Send SIGUSR1 to trigger heap dump (if jemalloc is configured for it)
    kill -USR1 "$PID" 2>/dev/null || echo "Warning: Could not send SIGUSR1 to $PID"
fi

# Start periodic memory stats collection
echo "Starting memory stats collection..."
(
    while true; do
        # Collect /proc stats
        if [ -f "/proc/$PID/status" ]; then
            echo "=== $(date -Iseconds) ===" >> "$OUTPUT_DIR/memory-stats-${NODE_NAME}.log"
            grep -E "^(VmRSS|VmHWM|VmSize|VmPeak|VmData|RssAnon|RssShmem)" "/proc/$PID/status" >> "$OUTPUT_DIR/memory-stats-${NODE_NAME}.log" 2>/dev/null || true
            
            # Also collect from /proc/meminfo
            echo "--- System Memory ---" >> "$OUTPUT_DIR/memory-stats-${NODE_NAME}.log"
            grep -E "^(MemTotal|MemFree|MemAvailable|Buffers|Cached)" /proc/meminfo >> "$OUTPUT_DIR/memory-stats-${NODE_NAME}.log" 2>/dev/null || true
            echo "" >> "$OUTPUT_DIR/memory-stats-${NODE_NAME}.log"
        fi
        sleep 30
    done
) &
MEMSTATS_PID=$!
echo "$MEMSTATS_PID" > "$OUTPUT_DIR/memstats-${NODE_NAME}.pid"
echo "Memory stats collection started with PID $MEMSTATS_PID"

echo ""
echo "Profiling started for $NODE_NAME"
echo "Output directory: $OUTPUT_DIR"
echo "PID files:"
[ "$ENABLE_PERF" = true ] && echo "  - perf: $OUTPUT_DIR/perf-${NODE_NAME}.pid"
echo "  - memstats: $OUTPUT_DIR/memstats-${NODE_NAME}.pid"
echo ""
echo "To stop profiling, run: stop-profiling.sh --node-name $NODE_NAME --output-dir $OUTPUT_DIR"

