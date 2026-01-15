#!/bin/bash
# Generate memory flamegraph from jemalloc heap dumps
# Usage: generate-memory-flamegraph.sh [OPTIONS]
#
# Options:
#   --input FILE        Input jemalloc heap dump file
#   --base FILE         Baseline heap dump for differential analysis (optional)
#   --binary FILE        Path to the binary (default: /usr/local/bin/merod)
#   --output FILE       Output SVG file
#   --title TITLE       Title for the flamegraph
#   --width WIDTH       Width of the SVG (default: 1200)
#   --colors SCHEME     Color scheme (mem, hot, io, red, green, blue, etc.)
#   --latest            Use the latest heap dump from input directory
#   --input-dir DIR     Directory to search for heap dumps (with --latest)

set -e

# Default values
INPUT=""
BASE=""
BINARY="/usr/local/bin/merod"
OUTPUT=""
TITLE="Memory Flamegraph"
WIDTH=1200
COLORS="mem"
USE_LATEST=false
INPUT_DIR=""
FLAMEGRAPH_DIR="${FLAMEGRAPH_DIR:-/opt/FlameGraph}"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --input)
            INPUT="$2"
            shift 2
            ;;
        --base)
            BASE="$2"
            shift 2
            ;;
        --binary)
            BINARY="$2"
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
        --latest)
            USE_LATEST=true
            shift
            ;;
        --input-dir)
            INPUT_DIR="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Find latest heap dump if requested
if [ "$USE_LATEST" = true ]; then
    if [ -z "$INPUT_DIR" ]; then
        INPUT_DIR="${PROFILING_OUTPUT_DIR:-/profiling/data}"
    fi
    
    INPUT=$(ls -t "$INPUT_DIR"/jemalloc*.heap 2>/dev/null | head -1)
    if [ -z "$INPUT" ]; then
        echo "Error: No heap dump files found in $INPUT_DIR"
        exit 1
    fi
    echo "Using latest heap dump: $INPUT"
fi

# Validate inputs
if [ -z "$INPUT" ]; then
    echo "Error: --input is required (or use --latest with --input-dir)"
    exit 1
fi

if [ ! -f "$INPUT" ]; then
    echo "Error: Input heap dump file not found: $INPUT"
    exit 1
fi

# Validate heap dump file (basic check - should be non-empty and readable)
if [ ! -s "$INPUT" ]; then
    echo "Error: Heap dump file is empty: $INPUT"
    exit 1
fi

# Basic validation: check if file looks like a jemalloc heap dump
# jemalloc heap dumps typically start with "heap_v2" or contain binary data
if ! head -c 100 "$INPUT" 2>/dev/null | grep -q "heap_v2\|MAPPED_LIBRARIES" 2>/dev/null; then
    # Not a text-based heap dump, might be binary - that's okay, continue
    # But check file size to ensure it's not completely empty
    FILE_SIZE=$(stat -f%z "$INPUT" 2>/dev/null || stat -c%s "$INPUT" 2>/dev/null || echo "0")
    if [ "$FILE_SIZE" -lt 100 ]; then
        echo "Warning: Heap dump file is very small ($FILE_SIZE bytes), may be invalid"
    fi
fi

if [ ! -f "$BINARY" ]; then
    echo "Error: Binary not found: $BINARY"
    echo "Note: jeprof needs the binary to resolve symbols"
    exit 1
fi

# Verify binary is executable
if [ ! -x "$BINARY" ]; then
    echo "Error: Binary is not executable: $BINARY"
    exit 1
fi

# Check for jeprof
JEPROF=$(which jeprof 2>/dev/null || echo "")
if [ -z "$JEPROF" ]; then
    # Try common locations
    if [ -f "/usr/local/bin/jeprof" ]; then
        JEPROF="/usr/local/bin/jeprof"
    elif [ -f "/usr/bin/jeprof" ]; then
        JEPROF="/usr/bin/jeprof"
    else
        echo "Error: jeprof not found in PATH"
        echo "jeprof should be installed with jemalloc"
        exit 1
    fi
fi

# Check for flamegraph.pl
FLAMEGRAPH="$FLAMEGRAPH_DIR/flamegraph.pl"
if [ ! -x "$FLAMEGRAPH" ]; then
    FLAMEGRAPH=$(which flamegraph.pl 2>/dev/null || true)
fi

if [ -z "$FLAMEGRAPH" ] || [ ! -x "$FLAMEGRAPH" ]; then
    echo "Error: flamegraph.pl not found"
    echo "Expected at: $FLAMEGRAPH_DIR or in PATH"
    exit 1
fi

# Auto-generate output filename if not specified
if [ -z "$OUTPUT" ]; then
    if [ -n "$BASE" ]; then
        OUTPUT="${INPUT%.heap}-diff.svg"
    else
        OUTPUT="${INPUT%.heap}.svg"
    fi
fi

# Create output directory if needed
mkdir -p "$(dirname "$OUTPUT")"

echo "Generating memory flamegraph..."
echo "  Input:  $INPUT"
[ -n "$BASE" ] && echo "  Base:   $BASE (differential analysis)"
echo "  Binary: $BINARY"
echo "  Output: $OUTPUT"
echo "  Title:  $TITLE"

# Generate the flamegraph using jeprof
FOLDED="${OUTPUT%.svg}.folded"
JEPROF_STDERR="${FOLDED}.stderr"

# Check if timeout command is available
if command -v timeout >/dev/null 2>&1; then
    TIMEOUT_CMD="timeout 600"  # 10 minute timeout for large heap dumps
    TIMEOUT_AVAILABLE=true
else
    TIMEOUT_CMD=""
    TIMEOUT_AVAILABLE=false
    echo "Warning: timeout command not available, jeprof may hang on very large heap dumps"
fi

# Cleanup function for error files
cleanup_on_error() {
    rm -f "$FOLDED" "$JEPROF_STDERR" 2>/dev/null || true
}

trap cleanup_on_error EXIT

echo "Converting heap dump to folded stacks with jeprof..."
if [ -n "$BASE" ]; then
    # Differential analysis: compare current heap to baseline
    if [ ! -f "$BASE" ]; then
        echo "Error: Baseline heap dump not found: $BASE"
        exit 1
    fi
    
    if [ ! -s "$BASE" ]; then
        echo "Error: Baseline heap dump is empty: $BASE"
        exit 1
    fi
    
    echo "  Performing differential analysis (showing memory growth)..."
    
    # Try differential analysis with proper error handling
    if [ "$TIMEOUT_AVAILABLE" = true ]; then
        if ! $TIMEOUT_CMD "$JEPROF" "$BINARY" --base "$BASE" "$INPUT" --collapse > "$FOLDED" 2>"$JEPROF_STDERR"; then
            JEPROF_EXIT_CODE=$?
            if [ "$JEPROF_EXIT_CODE" -eq 124 ]; then
                echo "Error: jeprof timed out after 10 minutes (heap dump may be too large)"
                [ -s "$JEPROF_STDERR" ] && echo "jeprof stderr:" && head -20 "$JEPROF_STDERR"
                exit 1
            fi
            
            echo "Warning: jeprof differential analysis failed (exit code: $JEPROF_EXIT_CODE)"
            if [ -s "$JEPROF_STDERR" ]; then
                echo "jeprof error output:"
                head -30 "$JEPROF_STDERR"
            fi
            echo "  Falling back to single heap dump analysis..."
            rm -f "$FOLDED" "$JEPROF_STDERR"
            
            # Fallback to single heap dump
            if ! $TIMEOUT_CMD "$JEPROF" "$BINARY" "$INPUT" --collapse > "$FOLDED" 2>"$JEPROF_STDERR"; then
                FALLBACK_EXIT=$?
                if [ "$FALLBACK_EXIT" -eq 124 ]; then
                    echo "Error: jeprof timed out on single heap dump analysis"
                    [ -s "$JEPROF_STDERR" ] && echo "jeprof stderr:" && head -20 "$JEPROF_STDERR"
                    exit 1
                fi
                echo "Error: Failed to process heap dump with jeprof (exit code: $FALLBACK_EXIT)"
                [ -s "$JEPROF_STDERR" ] && echo "jeprof stderr:" && cat "$JEPROF_STDERR"
                exit 1
            fi
        fi
    else
        # No timeout available
        if ! "$JEPROF" "$BINARY" --base "$BASE" "$INPUT" --collapse > "$FOLDED" 2>"$JEPROF_STDERR"; then
            echo "Warning: jeprof differential analysis failed"
            if [ -s "$JEPROF_STDERR" ]; then
                echo "jeprof error output:"
                head -30 "$JEPROF_STDERR"
            fi
            echo "  Falling back to single heap dump analysis..."
            rm -f "$FOLDED" "$JEPROF_STDERR"
            
            if ! "$JEPROF" "$BINARY" "$INPUT" --collapse > "$FOLDED" 2>"$JEPROF_STDERR"; then
                echo "Error: Failed to process heap dump with jeprof"
                [ -s "$JEPROF_STDERR" ] && echo "jeprof stderr:" && cat "$JEPROF_STDERR"
                exit 1
            fi
        fi
    fi
else
    # Single heap dump analysis
    if [ "$TIMEOUT_AVAILABLE" = true ]; then
        if ! $TIMEOUT_CMD "$JEPROF" "$BINARY" "$INPUT" --collapse > "$FOLDED" 2>"$JEPROF_STDERR"; then
            JEPROF_EXIT_CODE=$?
            if [ "$JEPROF_EXIT_CODE" -eq 124 ]; then
                echo "Error: jeprof timed out after 10 minutes (heap dump may be too large)"
                [ -s "$JEPROF_STDERR" ] && echo "jeprof stderr:" && head -20 "$JEPROF_STDERR"
                exit 1
            fi
            echo "Error: Failed to process heap dump with jeprof (exit code: $JEPROF_EXIT_CODE)"
            [ -s "$JEPROF_STDERR" ] && echo "jeprof stderr:" && cat "$JEPROF_STDERR"
            exit 1
        fi
    else
        if ! "$JEPROF" "$BINARY" "$INPUT" --collapse > "$FOLDED" 2>"$JEPROF_STDERR"; then
            echo "Error: Failed to process heap dump with jeprof"
            [ -s "$JEPROF_STDERR" ] && echo "jeprof stderr:" && cat "$JEPROF_STDERR"
            exit 1
        fi
    fi
fi

# Clean up stderr file if jeprof succeeded
rm -f "$JEPROF_STDERR"

if [ ! -s "$FOLDED" ]; then
    echo "Warning: No stack data in folded output. The heap dump may be empty or corrupt."
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

# Validate generated flamegraph files
if [ ! -s "$OUTPUT" ]; then
    echo "Error: Generated flamegraph is empty: $OUTPUT"
    rm -f "$FOLDED" "$OUTPUT" "$ICICLE_OUTPUT"
    exit 1
fi

if [ ! -s "$ICICLE_OUTPUT" ]; then
    echo "Warning: Generated icicle flamegraph is empty: $ICICLE_OUTPUT"
fi

# Cleanup intermediate file
rm -f "$FOLDED"

# Remove trap since we're done
trap - EXIT

echo ""
echo "Memory flamegraphs generated:"
echo "  - $OUTPUT"
echo "  - $ICICLE_OUTPUT"
echo ""
echo "File sizes:"
ls -lh "$OUTPUT" "$ICICLE_OUTPUT" 2>/dev/null || true

