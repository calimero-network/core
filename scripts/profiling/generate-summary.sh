#!/bin/bash
# Generate profiling summary markdown file
# Usage: generate-summary.sh <test-name> <data-dir> <reports-dir>
#
# Arguments:
#   test-name    Name of the test (e.g., kv-store, kv-store-with-handlers)
#   data-dir     Directory containing profiling data
#   reports-dir  Directory for profiling reports (output)

set -e

TEST_NAME="${1:?Error: test-name is required}"
DATA_DIR="${2:?Error: data-dir is required}"
REPORTS_DIR="${3:?Error: reports-dir is required}"

OUTPUT_FILE="$REPORTS_DIR/SUMMARY.md"

mkdir -p "$REPORTS_DIR"

{
    echo "## $TEST_NAME Profiling Summary"
    echo ""
    echo "Generated: $(date -Iseconds)"
    echo ""
    echo "### Files Collected"
    echo "\`\`\`"
    FILES=$(find "$DATA_DIR" -type f 2>/dev/null | head -50)
    if [ -z "$FILES" ]; then
        echo "No files found"
    else
        echo "$FILES"
    fi
    echo "\`\`\`"
    echo ""
    echo "### Reports Generated"
    echo ""
    echo "#### CPU Flamegraphs"
    echo "\`\`\`"
    CPU_FLAMEGRAPHS=$(find "$REPORTS_DIR" -type f -name "flamegraph*.svg" ! -name "*memory*" ! -name "*icicle*" 2>/dev/null | head -20)
    if [ -z "$CPU_FLAMEGRAPHS" ]; then
        echo "No CPU flamegraphs found"
    else
        echo "$CPU_FLAMEGRAPHS"
    fi
    echo "\`\`\`"
    echo ""
    echo "#### Memory Flamegraphs"
    echo "\`\`\`"
    MEM_FLAMEGRAPHS=$(find "$REPORTS_DIR" -type f -name "*memory-flamegraph*.svg" 2>/dev/null | head -20)
    if [ -z "$MEM_FLAMEGRAPHS" ]; then
        echo "No memory flamegraphs found"
    else
        echo "$MEM_FLAMEGRAPHS"
    fi
    echo "\`\`\`"
    echo ""
    echo "#### Memory Reports"
    echo "\`\`\`"
    MEM_REPORTS=$(find "$REPORTS_DIR" -type f -name "*memory-report*.txt" 2>/dev/null | head -20)
    if [ -z "$MEM_REPORTS" ]; then
        echo "No memory reports found"
    else
        echo "$MEM_REPORTS"
    fi
    echo "\`\`\`"
} > "$OUTPUT_FILE"

echo "Summary generated: $OUTPUT_FILE"

