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
# pipefail so we actually surface `perf script` failures instead of letting
# stackcollapse's zero exit code mask them (the May 14 DWARF run lost every
# CPU SVG silently because the pipeline returned 0 on empty input).
set -o pipefail

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
# Keep perf script's stderr (don't redirect to /dev/null) so the failure mode
# is visible next time. Persisted next to the output SVG.
PERF_SCRIPT_LOG="${OUTPUT%.svg}.perf-script.log"
# `--no-inline` skips per-frame addr2line subprocess invocation. With
# `--call-graph dwarf` perf script otherwise forks addr2line for every
# inline-expanded frame, and the May 12 attempt failed with
# `addr2line /usr/local/bin/merod: could not read first record` — which
# was the actual reason CPU flamegraphs disappeared. Built-in libbfd
# symbolization is still used; we just lose inline-function expansion
# (an acceptable trade for the SVG actually rendering).
set +e
perf script --no-inline -i "$INPUT" 2>"$PERF_SCRIPT_LOG" | "$STACKCOLLAPSE" > "$FOLDED"
PIPE_STATUS=("${PIPESTATUS[@]}")
set -e
if [ "${PIPE_STATUS[0]}" -ne 0 ] || [ "${PIPE_STATUS[1]}" -ne 0 ]; then
    echo "Warning: perf script/stackcollapse exited non-zero (perf=${PIPE_STATUS[0]}, stackcollapse=${PIPE_STATUS[1]})."
    echo "First lines of $PERF_SCRIPT_LOG:"
    head -10 "$PERF_SCRIPT_LOG" 2>/dev/null | sed 's/^/  /'
fi

if [ ! -s "$FOLDED" ]; then
    echo "Warning: No stack data captured. The perf data may be empty or corrupt."
    echo "Creating placeholder flamegraph..."
    # Encode the first stderr line into the placeholder so the rendered SVG
    # tells the next reader *why* it's empty instead of just saying "no_data".
    placeholder_msg=$(head -1 "$PERF_SCRIPT_LOG" 2>/dev/null | tr -c '[:alnum:]_ ' '_' | cut -c1-80)
    echo "no_data;${placeholder_msg:-empty_perf_script_output} 1" > "$FOLDED"
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

