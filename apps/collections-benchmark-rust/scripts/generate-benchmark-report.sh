#!/usr/bin/env sh
# Generate a formatted benchmark timing report for CRDT Collections
# Usage: generate-benchmark-report.sh [--output-file FILE]
# Timing data is read from environment variables (set automatically by workflow)

set -eu

OUTPUT_FILE=""

# Parse arguments
while [ $# -gt 0 ]; do
  case "$1" in
    --output-file)
      OUTPUT_FILE="$2"
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

# Function to get environment variable or default
get_env_or_default() {
  local var_value="$1"
  if [ -z "$var_value" ] || [ "$var_value" = "" ]; then
    echo "N/A"
  else
    echo "$var_value"
  fi
}

# Function to format duration
format_duration() {
  if [ "$1" = "N/A" ] || [ -z "$1" ]; then
    echo "N/A"
  else
    printf "%.3f" "$1" 2>/dev/null || echo "$1"
  fi
}

# Generate report
generate_report() {
  cat <<EOF
================================================================================
              CRDT COLLECTIONS BENCHMARK TIMING REPORT
                         Generated: $(date)
================================================================================

EXECUTIVE SUMMARY
--------------------------------------------------------------------------------
This report contains performance metrics for CRDT collection operations including:
- UnorderedMap operations (insert, get, nested, deep nested)
- Vector operations (push, get)
- UnorderedSet operations (insert, contains)
- LwwRegister operations (set, get)
- RGA (Replicated Growable Array) operations (insert, get_text)

All metrics are measured at the client level using Merobox timing features.

================================================================================
SECTION 1: UNORDERED MAP OPERATIONS
================================================================================

Operation: Map Insert (100 operations)
  Throughput:    $(get_env_or_default "${MAP_INSERT_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${MAP_INSERT_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${MAP_INSERT_DURATION:-N/A}") seconds

Operation: Map Get (50 operations)
  Throughput:    $(get_env_or_default "${MAP_GET_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${MAP_GET_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${MAP_GET_DURATION:-N/A}") seconds

================================================================================
SECTION 2: NESTED MAP OPERATIONS (Level 2)
================================================================================

Operation: Nested Map Insert (50 operations)
  Throughput:    $(get_env_or_default "${NESTED_MAP_INSERT_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${NESTED_MAP_INSERT_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${NESTED_MAP_INSERT_DURATION:-N/A}") seconds

Operation: Nested Map Get (25 operations)
  Throughput:    $(get_env_or_default "${NESTED_MAP_GET_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${NESTED_MAP_GET_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${NESTED_MAP_GET_DURATION:-N/A}") seconds

================================================================================
SECTION 3: DEEP NESTED MAP OPERATIONS (Level 3)
================================================================================

Operation: Deep Nested Map Insert (30 operations)
  Throughput:    $(get_env_or_default "${DEEP_NESTED_MAP_INSERT_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${DEEP_NESTED_MAP_INSERT_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${DEEP_NESTED_MAP_INSERT_DURATION:-N/A}") seconds

Operation: Deep Nested Map Get (15 operations)
  Throughput:    $(get_env_or_default "${DEEP_NESTED_MAP_GET_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${DEEP_NESTED_MAP_GET_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${DEEP_NESTED_MAP_GET_DURATION:-N/A}") seconds

================================================================================
SECTION 4: VECTOR OPERATIONS
================================================================================

Operation: Vector Push (100 operations)
  Throughput:    $(get_env_or_default "${VECTOR_PUSH_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${VECTOR_PUSH_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${VECTOR_PUSH_DURATION:-N/A}") seconds

Operation: Vector Get (50 operations)
  Throughput:    $(get_env_or_default "${VECTOR_GET_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${VECTOR_GET_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${VECTOR_GET_DURATION:-N/A}") seconds

================================================================================
SECTION 5: UNORDERED SET OPERATIONS
================================================================================

Operation: Set Insert (100 operations)
  Throughput:    $(get_env_or_default "${SET_INSERT_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${SET_INSERT_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${SET_INSERT_DURATION:-N/A}") seconds

Operation: Set Contains (50 operations)
  Throughput:    $(get_env_or_default "${SET_CONTAINS_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${SET_CONTAINS_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${SET_CONTAINS_DURATION:-N/A}") seconds

================================================================================
SECTION 6: LWW REGISTER OPERATIONS
================================================================================

Operation: Register Set (100 operations)
  Throughput:    $(get_env_or_default "${REGISTER_SET_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${REGISTER_SET_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${REGISTER_SET_DURATION:-N/A}") seconds

Operation: Register Get (50 operations)
  Throughput:    $(get_env_or_default "${REGISTER_GET_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${REGISTER_GET_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${REGISTER_GET_DURATION:-N/A}") seconds

================================================================================
SECTION 7: RGA (REPLICATED GROWABLE ARRAY) OPERATIONS
================================================================================

Operation: RGA Insert (50 operations)
  Throughput:    $(get_env_or_default "${RGA_INSERT_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${RGA_INSERT_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${RGA_INSERT_DURATION:-N/A}") seconds

Operation: RGA Get Text (10 operations)
  Throughput:    $(get_env_or_default "${RGA_GET_TEXT_THROUGHPUT:-N/A}") operations/second
  Avg Latency:   $(get_env_or_default "${RGA_GET_TEXT_AVG_LATENCY:-N/A}") milliseconds/operation
  Total Duration: $(format_duration "${RGA_GET_TEXT_DURATION:-N/A}") seconds

================================================================================
SECTION 8: PERFORMANCE SUMMARY
================================================================================

Operation Counts:
  Map Operations:        $(get_env_or_default "${MAP_FINAL_COUNT:-N/A}") total operations
  Nested Map Operations: $(get_env_or_default "${NESTED_MAP_FINAL_COUNT:-N/A}") total operations
  Deep Nested Map Ops:   $(get_env_or_default "${DEEP_NESTED_MAP_FINAL_COUNT:-N/A}") total operations
  Vector Operations:     $(get_env_or_default "${VECTOR_FINAL_COUNT:-N/A}") total operations
  Set Operations:        $(get_env_or_default "${SET_FINAL_COUNT:-N/A}") total operations
  Register Operations:   $(get_env_or_default "${REGISTER_FINAL_COUNT:-N/A}") total operations
  RGA Operations:        $(get_env_or_default "${RGA_FINAL_COUNT:-N/A}") total operations
  Final Total:           $(get_env_or_default "${FINAL_OPERATION_COUNT:-N/A}") total operations

================================================================================
END OF REPORT
================================================================================
EOF
}

# Generate the report
REPORT=$(generate_report)

# Always output to console
echo "$REPORT"

# Save to file if specified
if [ -n "$OUTPUT_FILE" ]; then
  echo "$REPORT" > "$OUTPUT_FILE"
  echo "Report saved to: $OUTPUT_FILE" >&2
fi
