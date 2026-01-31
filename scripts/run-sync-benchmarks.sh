#!/bin/bash
# ============================================================================
# Sync Strategy Benchmark Runner
# ============================================================================
#
# This script runs comprehensive benchmarks comparing different sync strategies.
#
# Usage:
#   ./scripts/run-sync-benchmarks.sh [options]
#
# Options:
#   --snapshot-only    Only run snapshot benchmark
#   --delta-only       Only run delta benchmark
#   --quick            Reduce wait times (for CI)
#   --help             Show this help
#
# Requirements:
#   - merobox installed (pip install -e /path/to/merobox)
#   - merod binary built (cargo build --release -p merod)
#
# ============================================================================

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
MEROD_BINARY="${PROJECT_ROOT}/target/release/merod"
RESULTS_DIR="${PROJECT_ROOT}/benchmark-results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)

# Parse arguments
RUN_SNAPSHOT=true
RUN_DELTA=true
QUICK_MODE=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --snapshot-only)
            RUN_DELTA=false
            shift
            ;;
        --delta-only)
            RUN_SNAPSHOT=false
            shift
            ;;
        --quick)
            QUICK_MODE=true
            shift
            ;;
        --help)
            head -30 "$0" | tail -25
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            exit 1
            ;;
    esac
done

# Functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check prerequisites
check_prerequisites() {
    log_info "Checking prerequisites..."
    
    if [[ ! -f "$MEROD_BINARY" ]]; then
        log_warn "merod binary not found at $MEROD_BINARY"
        log_info "Building merod in release mode..."
        cd "$PROJECT_ROOT"
        cargo build --release -p merod
    fi
    
    if ! command -v merobox &> /dev/null && ! python -m merobox.cli --help &> /dev/null 2>&1; then
        log_error "merobox not found. Install with: pip install -e /path/to/merobox"
        exit 1
    fi
    
    log_success "Prerequisites OK"
}

# Clean up previous runs
cleanup() {
    log_info "Cleaning up previous benchmark data..."
    rm -rf "${PROJECT_ROOT}/data/bench-snap-"* 2>/dev/null || true
    rm -rf "${PROJECT_ROOT}/data/bench-delta-"* 2>/dev/null || true
}

# Run a benchmark and capture output
run_benchmark() {
    local name=$1
    local workflow=$2
    local merod_args=$3
    local log_file="${RESULTS_DIR}/${TIMESTAMP}_${name}.log"
    
    log_info "Running benchmark: ${name}"
    log_info "  Workflow: ${workflow}"
    log_info "  merod args: ${merod_args}"
    log_info "  Log file: ${log_file}"
    
    mkdir -p "$RESULTS_DIR"
    
    # Clean up data directories for this benchmark
    local prefix=$(echo "$workflow" | sed 's/.*bench-/bench-/' | sed 's/\.yml//')
    rm -rf "${PROJECT_ROOT}/data/${prefix}-"* 2>/dev/null || true
    
    # Run the benchmark
    local start_time=$(date +%s.%N)
    
    if python -m merobox.cli bootstrap run \
        --no-docker \
        --binary-path "$MEROD_BINARY" \
        --merod-args="$merod_args" \
        "${PROJECT_ROOT}/${workflow}" 2>&1 | tee "$log_file"; then
        
        local end_time=$(date +%s.%N)
        local duration=$(echo "$end_time - $start_time" | bc)
        
        log_success "Benchmark ${name} completed in ${duration}s"
        
        # Extract key metrics from log
        extract_metrics "$log_file" "$name"
        
        return 0
    else
        local end_time=$(date +%s.%N)
        local duration=$(echo "$end_time - $start_time" | bc)
        
        log_error "Benchmark ${name} FAILED after ${duration}s"
        return 1
    fi
}

# Extract metrics from log file
extract_metrics() {
    local log_file=$1
    local name=$2
    
    echo ""
    echo "=========================================="
    echo "METRICS: $name"
    echo "=========================================="
    
    # Extract sync timing info
    if grep -q "Snapshot sync completed" "$log_file"; then
        echo "Snapshot Sync Timings:"
        grep "Snapshot sync completed" "$log_file" | grep -oE "duration_ms=[0-9.]+" | head -5
    fi
    
    if grep -q "Sync finished successfully" "$log_file"; then
        echo "Overall Sync Timings:"
        grep "Sync finished successfully" "$log_file" | grep -oE "duration_ms=[0-9.]+" | head -5
    fi
    
    # Count deltas if delta sync
    local delta_count=$(grep -c "request_delta\|Delta applied" "$log_file" 2>/dev/null || echo "0")
    if [[ "$delta_count" -gt 0 ]]; then
        echo "Delta operations: $delta_count"
    fi
    
    # Check for failures
    local failures=$(grep -c "FAILED\|error\|panic" "$log_file" 2>/dev/null || echo "0")
    if [[ "$failures" -gt 0 ]]; then
        echo "Warnings/Errors found: $failures (check log for details)"
    fi
    
    echo "=========================================="
    echo ""
}

# Generate summary report
generate_summary() {
    local summary_file="${RESULTS_DIR}/${TIMESTAMP}_summary.txt"
    
    echo "=========================================="
    echo "BENCHMARK SUMMARY"
    echo "=========================================="
    echo "Timestamp: $(date)"
    echo "Results directory: $RESULTS_DIR"
    echo ""
    
    # List all benchmark logs from this run
    for log in "${RESULTS_DIR}/${TIMESTAMP}_"*.log; do
        if [[ -f "$log" ]]; then
            local name=$(basename "$log" .log | sed "s/${TIMESTAMP}_//")
            echo "--- $name ---"
            
            # Quick stats
            if grep -q "Snapshot sync completed" "$log"; then
                grep "Snapshot sync completed" "$log" | tail -1
            fi
            if grep -q "Sync finished successfully" "$log"; then
                grep "Sync finished successfully" "$log" | tail -1
            fi
            echo ""
        fi
    done
    
    echo "==========================================" | tee -a "$summary_file"
}

# Main execution
main() {
    echo ""
    echo "=============================================="
    echo "  CALIMERO SYNC STRATEGY BENCHMARKS"
    echo "=============================================="
    echo ""
    
    check_prerequisites
    cleanup
    
    local failed=0
    
    # Run snapshot benchmark
    if [[ "$RUN_SNAPSHOT" == "true" ]]; then
        if ! run_benchmark "snapshot" \
            "workflows/sync/bench-fresh-node-snapshot.yml" \
            "--sync-strategy snapshot"; then
            failed=$((failed + 1))
        fi
    fi
    
    # Clean between runs
    cleanup
    
    # Run delta benchmark
    if [[ "$RUN_DELTA" == "true" ]]; then
        if ! run_benchmark "delta" \
            "workflows/sync/bench-fresh-node-delta.yml" \
            "--sync-strategy delta"; then
            failed=$((failed + 1))
        fi
    fi
    
    # Generate summary
    echo ""
    generate_summary
    
    if [[ $failed -gt 0 ]]; then
        log_error "$failed benchmark(s) failed"
        exit 1
    else
        log_success "All benchmarks completed successfully!"
    fi
}

# Run main
main "$@"
