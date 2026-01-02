#!/bin/bash
# Generate CPU flamegraph from perf data
# Usage: generate-flamegraph.sh [OPTIONS]
#
# Options:
#   --input FILE        Input perf.data file
#   --output FILE       Output SVG file
#   --title TITLE       Title for the flamegraph
#   --width WIDTH       Width of the SVG (default: 1200)
#   --colors SCHEME     Color scheme (hot, mem, io, red, green, blue, etc.)

set -e

# Default values
INPUT=""
OUTPUT=""
TITLE="CPU Flamegraph"
WIDTH=1200
COLORS="hot"
FLAMEGRAPH_DIR="${FLAMEGRAPH_DIR:-/opt/FlameGraph}"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --input)
            INPUT="$2"
            shift 2
            ;;
        --output)
            OUTPUT="$2"
            shift 2
            ;;
        --title)
            TITLE="$2"
            shift 2
            ;;
        --width)
            WIDTH="$2"
            shift 2
            ;;
        --colors)
            COLORS="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Validate inputs
if [ -z "$INPUT" ]; then
    echo "Error: --input is required"
    exit 1
fi

if [ ! -f "$INPUT" ]; then
    echo "Error: Input file not found: $INPUT"
    exit 1
fi

# Auto-generate output filename if not specified
if [ -z "$OUTPUT" ]; then
    OUTPUT="${INPUT%.data}.svg"
fi

# Create output directory if needed
mkdir -p "$(dirname "$OUTPUT")"

echo "Generating flamegraph..."
echo "  Input:  $INPUT"
echo "  Output: $OUTPUT"
echo "  Title:  $TITLE"

# Check for FlameGraph tools
STACKCOLLAPSE="$FLAMEGRAPH_DIR/stackcollapse-perf.pl"
FLAMEGRAPH="$FLAMEGRAPH_DIR/flamegraph.pl"

if [ ! -x "$STACKCOLLAPSE" ]; then
    STACKCOLLAPSE=$(which stackcollapse-perf.pl 2>/dev/null || true)
fi

if [ ! -x "$FLAMEGRAPH" ]; then
    FLAMEGRAPH=$(which flamegraph.pl 2>/dev/null || true)
fi

if [ -z "$STACKCOLLAPSE" ] || [ -z "$FLAMEGRAPH" ]; then
    echo "Error: FlameGraph tools not found"
    echo "Expected at: $FLAMEGRAPH_DIR or in PATH"
    exit 1
fi

# Generate the flamegraph
FOLDED="${OUTPUT%.svg}.folded"

# Restore perf.map files if they exist (for WASM JIT code symbolization)
# perf script automatically looks for /tmp/perf-<pid>.map files
PERF_MAP_DIR="${PROFILING_OUTPUT_DIR:-/profiling/data}"
if [ -d "$PERF_MAP_DIR" ]; then
    # Find any preserved perf.map files and restore them to /tmp
    for perf_map_file in "$PERF_MAP_DIR"/perf-*.map; do
        if [ -f "$perf_map_file" ]; then
            MAP_BASENAME=$(basename "$perf_map_file")
            RESTORE_PID=""
            
            # Try format: perf-<name>-<pid>.map (e.g., perf-fuzzy-kv-node-1-196.map)
            if [[ "$MAP_BASENAME" =~ perf-.*-([0-9]+)\.map ]]; then
                RESTORE_PID="${BASH_REMATCH[1]}"
            # Try format: perf-<pid>.map (direct from Wasmer, e.g., perf-196.map)
            elif [[ "$MAP_BASENAME" =~ ^perf-([0-9]+)\.map$ ]]; then
                RESTORE_PID="${BASH_REMATCH[1]}"
            fi
            
            if [ -n "$RESTORE_PID" ]; then
                RESTORE_TARGET="/tmp/perf-${RESTORE_PID}.map"
                if [ ! -f "$RESTORE_TARGET" ]; then
                    echo "Restoring perf.map file for PID $RESTORE_PID (for WASM symbolization)..."
                    cp "$perf_map_file" "$RESTORE_TARGET" 2>/dev/null || true
                fi
            fi
        fi
    done
fi

echo "Converting perf data to folded stacks..."
perf script -i "$INPUT" 2>/dev/null | "$STACKCOLLAPSE" > "$FOLDED"

if [ ! -s "$FOLDED" ]; then
    echo "Warning: No stack data captured. The perf data may be empty or corrupt."
    echo "Creating placeholder flamegraph..."
    echo "no_data 1" > "$FOLDED"
fi

echo "Generating SVG flamegraph..."
"$FLAMEGRAPH" \
    --title "$TITLE" \
    --width "$WIDTH" \
    --colors "$COLORS" \
    --hash \
    "$FOLDED" > "$OUTPUT"

# Also generate a reversed (icicle) graph
ICICLE_OUTPUT="${OUTPUT%.svg}-icicle.svg"
echo "Generating icicle graph..."
"$FLAMEGRAPH" \
    --title "$TITLE (Icicle)" \
    --width "$WIDTH" \
    --colors "$COLORS" \
    --hash \
    --reverse \
    --inverted \
    "$FOLDED" > "$ICICLE_OUTPUT"

# Cleanup intermediate file
rm -f "$FOLDED"

echo ""
echo "Flamegraphs generated:"
echo "  - $OUTPUT"
echo "  - $ICICLE_OUTPUT"
echo ""
echo "File sizes:"
ls -lh "$OUTPUT" "$ICICLE_OUTPUT" 2>/dev/null || true

