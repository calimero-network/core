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
    find "$DATA_DIR" -type f 2>/dev/null | head -50 || echo "No files found"
    echo "\`\`\`"
    echo ""
    echo "### Reports Generated"
    echo "\`\`\`"
    find "$REPORTS_DIR" -type f \( -name "*.svg" -o -name "*.txt" \) 2>/dev/null || echo "No reports found"
    echo "\`\`\`"
} > "$OUTPUT_FILE"

echo "Summary generated: $OUTPUT_FILE"

