#!/bin/bash
# Generate memory profiling report
# Usage: generate-memory-report.sh [OPTIONS]
#
# Options:
#   --node-name NAME    Name of the node
#   --input-dir DIR     Directory containing profiling data
#   --output FILE       Output report file
#   --format FORMAT     Output format (text, json, html) - default: text

set -e

# Default values
NODE_NAME="merod"
INPUT_DIR="${PROFILING_OUTPUT_DIR:-/profiling/data}"
OUTPUT=""
FORMAT="text"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --node-name)
            NODE_NAME="$2"
            shift 2
            ;;
        --input-dir)
            INPUT_DIR="$2"
            shift 2
            ;;
        --output)
            OUTPUT="$2"
            shift 2
            ;;
        --format)
            FORMAT="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Auto-generate output filename if not specified
if [ -z "$OUTPUT" ]; then
    OUTPUT="${PROFILING_REPORTS_DIR:-/profiling/reports}/memory-report-${NODE_NAME}.txt"
fi

# Create output directory if needed
mkdir -p "$(dirname "$OUTPUT")"

echo "Generating memory report for $NODE_NAME..."
echo "  Input dir: $INPUT_DIR"
echo "  Output:    $OUTPUT"

# Start report
{
    echo "========================================"
    echo "Memory Profiling Report"
    echo "Node: $NODE_NAME"
    echo "Generated: $(date -Iseconds)"
    echo "========================================"
    echo ""
    
    # Process memory stats log
    MEMSTATS_LOG="$INPUT_DIR/memory-stats-${NODE_NAME}.log"
    if [ -f "$MEMSTATS_LOG" ]; then
        echo "### Memory Statistics Over Time ###"
        echo ""
        
        # Extract peak values
        echo "Peak Memory Values:"
        echo "-------------------"
        
        PEAK_RSS=$(grep "VmHWM:" "$MEMSTATS_LOG" 2>/dev/null | awk '{print $2}' | sort -n | tail -1)
        PEAK_VSZ=$(grep "VmPeak:" "$MEMSTATS_LOG" 2>/dev/null | awk '{print $2}' | sort -n | tail -1)
        FINAL_RSS=$(grep "VmRSS:" "$MEMSTATS_LOG" 2>/dev/null | tail -1 | awk '{print $2}')
        FINAL_VSZ=$(grep "VmSize:" "$MEMSTATS_LOG" 2>/dev/null | tail -1 | awk '{print $2}')
        
        echo "  Peak RSS (VmHWM):    ${PEAK_RSS:-N/A} kB"
        echo "  Peak Virtual (VmPeak): ${PEAK_VSZ:-N/A} kB"
        echo "  Final RSS:           ${FINAL_RSS:-N/A} kB"
        echo "  Final Virtual:       ${FINAL_VSZ:-N/A} kB"
        echo ""
        
        # Calculate growth
        FIRST_RSS=$(grep "VmRSS:" "$MEMSTATS_LOG" 2>/dev/null | head -1 | awk '{print $2}')
        if [ -n "$FIRST_RSS" ] && [ -n "$FINAL_RSS" ]; then
            GROWTH=$((FINAL_RSS - FIRST_RSS))
            echo "RSS Growth: $GROWTH kB (from $FIRST_RSS kB to $FINAL_RSS kB)"
            echo ""
        fi
        
        # Show timeline summary
        echo "Timeline Summary (sampled every 30s):"
        echo "-------------------------------------"
        grep -E "^(===|VmRSS:)" "$MEMSTATS_LOG" 2>/dev/null | head -40 || echo "No timeline data"
        echo "..."
        echo "(truncated, see full log for details)"
        echo ""
    else
        echo "No memory stats log found at: $MEMSTATS_LOG"
        echo ""
    fi
    
    # Process jemalloc heap dumps
    echo "### jemalloc Heap Dumps ###"
    echo ""
    HEAP_DUMPS=$(ls -1 "$INPUT_DIR"/jemalloc*.heap 2>/dev/null || true)
    if [ -n "$HEAP_DUMPS" ]; then
        echo "Found heap dumps:"
        for dump in $HEAP_DUMPS; do
            echo "  - $(basename "$dump") ($(stat -c%s "$dump" 2>/dev/null || echo "?") bytes)"
        done
        echo ""
        
        # If jeprof is available, generate analysis
        if command -v jeprof &>/dev/null; then
            echo "Analyzing most recent heap dump..."
            LATEST_DUMP=$(ls -t "$INPUT_DIR"/jemalloc*.heap 2>/dev/null | head -1)
            if [ -n "$LATEST_DUMP" ]; then
                jeprof --text /usr/local/bin/merod "$LATEST_DUMP" 2>/dev/null | head -50 || echo "Could not analyze dump"
            fi
        else
            echo "(jeprof not available for detailed heap analysis)"
        fi
        echo ""
    else
        echo "No jemalloc heap dumps found"
        echo ""
    fi
    
    # Process heaptrack data
    echo "### Heaptrack Analysis ###"
    echo ""
    HEAPTRACK_FILES=$(ls -1 "$INPUT_DIR"/heaptrack-${NODE_NAME}*.zst 2>/dev/null || ls -1 "$INPUT_DIR"/heaptrack-${NODE_NAME}*.gz 2>/dev/null || true)
    if [ -n "$HEAPTRACK_FILES" ]; then
        echo "Found heaptrack data:"
        for f in $HEAPTRACK_FILES; do
            echo "  - $(basename "$f")"
        done
        echo ""
        
        # If heaptrack_print is available, generate summary
        if command -v heaptrack_print &>/dev/null; then
            LATEST_HT=$(ls -t "$INPUT_DIR"/heaptrack-${NODE_NAME}* 2>/dev/null | head -1)
            if [ -n "$LATEST_HT" ]; then
                echo "Summary from heaptrack_print:"
                echo "-----------------------------"
                heaptrack_print -T "$LATEST_HT" 2>/dev/null | head -100 || echo "Could not analyze heaptrack data"
            fi
        else
            echo "(heaptrack_print not available - use heaptrack_gui for detailed analysis)"
        fi
        echo ""
    else
        echo "No heaptrack data found"
        echo ""
    fi
    
    # System information
    echo "### System Information ###"
    echo ""
    echo "Kernel: $(uname -r)"
    echo "Total RAM: $(grep MemTotal /proc/meminfo 2>/dev/null | awk '{print $2 " " $3}')"
    echo ""
    
    echo "========================================"
    echo "End of Memory Report"
    echo "========================================"
    
} > "$OUTPUT"

echo ""
echo "Memory report generated: $OUTPUT"
echo ""

# Show summary
echo "Report Summary:"
echo "---------------"
head -30 "$OUTPUT"
echo "..."
echo "(see full report at $OUTPUT)"

