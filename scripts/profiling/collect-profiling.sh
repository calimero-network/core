#!/bin/bash
# Collect and package all profiling data from a node
# Usage: collect-profiling.sh [OPTIONS]
#
# Options:
#   --node NAME    Name of the node
#   --input-dir DIR     Directory containing profiling data
#   --output-dir DIR    Directory to store collected/packaged data
#   --archive           Create a compressed archive of all data

set -e

# Default values
NODE_NAME="merod"
INPUT_DIR="${PROFILING_OUTPUT_DIR:-/profiling/data}"
OUTPUT_DIR="${PROFILING_REPORTS_DIR:-/profiling/reports}"
CREATE_ARCHIVE=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --node)
            NODE_NAME="$2"
            shift 2
            ;;
        --input-dir)
            INPUT_DIR="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --archive)
            CREATE_ARCHIVE=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo "Collecting profiling data for $NODE_NAME..."
echo "  Input:  $INPUT_DIR"
echo "  Output: $OUTPUT_DIR"

# Create output directory
mkdir -p "$OUTPUT_DIR/$NODE_NAME"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
COLLECT_DIR="$OUTPUT_DIR/$NODE_NAME"

# Stop any running profiling first
/profiling/scripts/stop-profiling.sh --node "$NODE_NAME" --output-dir "$INPUT_DIR" || true

# Copy all relevant data files
echo ""
echo "Collecting data files..."

# perf data
for f in "$INPUT_DIR"/perf-${NODE_NAME}*.data; do
    if [ -f "$f" ]; then
        echo "  - $(basename "$f")"
        cp "$f" "$COLLECT_DIR/"
    fi
done

# Memory stats
for f in "$INPUT_DIR"/memory-stats-${NODE_NAME}*.log; do
    if [ -f "$f" ]; then
        echo "  - $(basename "$f")"
        cp "$f" "$COLLECT_DIR/"
    fi
done

# jemalloc heap dumps
for f in "$INPUT_DIR"/jemalloc*.heap; do
    if [ -f "$f" ]; then
        echo "  - $(basename "$f")"
        cp "$f" "$COLLECT_DIR/"
    fi
done

# heaptrack data
for f in "$INPUT_DIR"/heaptrack-${NODE_NAME}*; do
    if [ -f "$f" ]; then
        echo "  - $(basename "$f")"
        cp "$f" "$COLLECT_DIR/"
    fi
done

# Generate reports
echo ""
echo "Generating reports..."

# Generate flamegraph if perf data exists
PERF_DATA=$(ls -t "$COLLECT_DIR"/perf-${NODE_NAME}*.data 2>/dev/null | head -1)
if [ -n "$PERF_DATA" ] && [ -f "$PERF_DATA" ]; then
    echo "Generating flamegraph..."
    /profiling/scripts/generate-flamegraph.sh \
        --input "$PERF_DATA" \
        --output "$COLLECT_DIR/flamegraph-${NODE_NAME}.svg" \
        --title "CPU Flamegraph - $NODE_NAME" || echo "Warning: Flamegraph generation failed"
fi

# Generate memory report
echo "Generating memory report..."
/profiling/scripts/generate-memory-report.sh \
    --node "$NODE_NAME" \
    --input-dir "$INPUT_DIR" \
    --output "$COLLECT_DIR/memory-report-${NODE_NAME}.txt" || echo "Warning: Memory report generation failed"

# Generate memory flamegraph from jemalloc heap dumps
echo "Generating memory flamegraph..."
HEAP_DUMP=$(ls -t "$INPUT_DIR"/jemalloc*.heap 2>/dev/null | head -1)
if [ -n "$HEAP_DUMP" ]; then
    echo "  Found heap dump: $(basename "$HEAP_DUMP")"
    
    # Try to find a baseline (first heap dump) for differential analysis
    BASELINE_HEAP=$(ls -t "$INPUT_DIR"/jemalloc*.heap 2>/dev/null | tail -1)
    if [ -n "$BASELINE_HEAP" ] && [ "$BASELINE_HEAP" != "$HEAP_DUMP" ]; then
        echo "  Using baseline: $(basename "$BASELINE_HEAP") for differential analysis"
        /profiling/scripts/generate-memory-flamegraph.sh \
            --input "$HEAP_DUMP" \
            --base "$BASELINE_HEAP" \
            --output "$COLLECT_DIR/memory-flamegraph-${NODE_NAME}.svg" \
            --title "Memory Flamegraph (Diff) - $NODE_NAME" \
            --colors mem || echo "Warning: Memory flamegraph generation failed"
    else
        # Single heap dump analysis
        /profiling/scripts/generate-memory-flamegraph.sh \
            --input "$HEAP_DUMP" \
            --output "$COLLECT_DIR/memory-flamegraph-${NODE_NAME}.svg" \
            --title "Memory Flamegraph - $NODE_NAME" \
            --colors mem || echo "Warning: Memory flamegraph generation failed"
    fi
else
    echo "  No heap dump files found"
fi

# Create summary file
echo ""
echo "Creating summary..."
{
    echo "Profiling Data Collection Summary"
    echo "================================="
    echo "Node:      $NODE_NAME"
    echo "Timestamp: $TIMESTAMP"
    echo "Collected: $(date -Iseconds)"
    echo ""
    echo "Files Collected:"
    echo "----------------"
    ls -lh "$COLLECT_DIR"/ 2>/dev/null
    echo ""
    echo "Total Size: $(du -sh "$COLLECT_DIR" 2>/dev/null | cut -f1)"
} > "$COLLECT_DIR/SUMMARY.txt"

cat "$COLLECT_DIR/SUMMARY.txt"

# Create archive if requested
if [ "$CREATE_ARCHIVE" = true ]; then
    echo ""
    echo "Creating archive..."
    ARCHIVE_NAME="profiling-${NODE_NAME}-${TIMESTAMP}.tar.gz"
    tar -czf "$OUTPUT_DIR/$ARCHIVE_NAME" -C "$OUTPUT_DIR" "$NODE_NAME"
    echo "Archive created: $OUTPUT_DIR/$ARCHIVE_NAME"
    echo "Archive size: $(ls -lh "$OUTPUT_DIR/$ARCHIVE_NAME" | awk '{print $5}')"
fi

echo ""
echo "Collection complete!"
echo "Data directory: $COLLECT_DIR"

