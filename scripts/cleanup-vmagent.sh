#!/bin/bash
# Cleanup vmagent and background processes
# Usage: cleanup-vmagent.sh <test_case> <update_pid> <vmagent_pid> <vmagent_config> <vmagent_log> [exit_code] [http_port]
#
# With exit_code and http_port, first pushes the run's outcome through vmagent
# so dashboards can show pass/fail per commit:
#   ci_test_passed{<run labels>}            1 or 0
#   ci_test_exit_code{<run labels>}         the test command's exit code
#   ci_test_duration_seconds{<run labels>}  seconds since run-vmagent.sh started

set -euo pipefail

TEST_CASE="${1:-}"
UPDATE_PID="${2:-}"
VMAGENT_PID="${3:-}"
VMAGENT_CONFIG="${4:-}"
VMAGENT_LOG="${5:-}"
EXIT_CODE="${6:-}"
HTTP_PORT="${7:-}"
# Where setup-vmagent.sh / run-vmagent.sh keep per-run state.
VMAGENT_DIR="/tmp/vmagent-${TEST_CASE}"

if [ -z "$TEST_CASE" ]; then
    echo "Usage: $0 <test_case> <update_pid> <vmagent_pid> <vmagent_config> <vmagent_log> [exit_code] [http_port]"
    exit 1
fi

echo "Cleaning up vmagent for test case: $TEST_CASE"

# Push the run's outcome. Best-effort: a failed push must never fail the job
# or mask the test's own exit code.
if [ -n "$EXIT_CODE" ] && [ -n "$HTTP_PORT" ] && [ -f "$VMAGENT_DIR/run_labels" ] &&
    [ -n "$VMAGENT_PID" ] && kill -0 "$VMAGENT_PID" 2>/dev/null; then
    LABELS=$(cat "$VMAGENT_DIR/run_labels")
    PASSED=0
    [ "$EXIT_CODE" = "0" ] && PASSED=1
    DURATION=0
    if [ -f "$VMAGENT_DIR/run_start" ]; then
        DURATION=$(( $(date +%s) - $(cat "$VMAGENT_DIR/run_start") ))
    fi
    if printf 'ci_test_passed{%s} %s\nci_test_exit_code{%s} %s\nci_test_duration_seconds{%s} %s\n' \
        "$LABELS" "$PASSED" "$LABELS" "$EXIT_CODE" "$LABELS" "$DURATION" |
        curl -fsS --max-time 10 --data-binary @- \
            "http://127.0.0.1:${HTTP_PORT}/api/v1/import/prometheus"; then
        echo "Pushed run result: passed=$PASSED exit_code=$EXIT_CODE duration=${DURATION}s"
    else
        echo "WARNING: failed to push run result to vmagent" >&2
    fi
fi

# Final config update to capture all processes (if config file provided)
if [ -n "$VMAGENT_CONFIG" ] && [ -f "$VMAGENT_CONFIG" ] && [ -n "$VMAGENT_PID" ]; then
    if kill -0 "$VMAGENT_PID" 2>/dev/null; then
        echo "Performing final config update..."
        # Signal vmagent to reload config one last time
        kill -HUP "$VMAGENT_PID" 2>/dev/null || true
    fi
fi

# Wait a bit for final metrics to be sent
if [ -n "$VMAGENT_PID" ] && kill -0 "$VMAGENT_PID" 2>/dev/null; then
    echo "Waiting for final metrics to be sent..."
    sleep 10
fi

# Stop background updater process
if [ -n "$UPDATE_PID" ] && kill -0 "$UPDATE_PID" 2>/dev/null; then
    echo "Stopping config updater (PID: $UPDATE_PID)..."
    kill "$UPDATE_PID" 2>/dev/null || true
    # Poll until process terminates (max 10 seconds)
    count=0
    while kill -0 "$UPDATE_PID" 2>/dev/null && [ $count -lt 20 ]; do
        sleep 0.5
        count=$((count + 1))
    done
    if kill -0 "$UPDATE_PID" 2>/dev/null; then
        echo "WARNING: Config updater (PID: $UPDATE_PID) did not terminate within 10 seconds"
    fi
fi

# Stop vmagent
if [ -n "$VMAGENT_PID" ] && kill -0 "$VMAGENT_PID" 2>/dev/null; then
    echo "Stopping vmagent (PID: $VMAGENT_PID)..."
    kill "$VMAGENT_PID" 2>/dev/null || true
    # Poll until process terminates (max 10 seconds)
    count=0
    while kill -0 "$VMAGENT_PID" 2>/dev/null && [ $count -lt 20 ]; do
        sleep 0.5
        count=$((count + 1))
    done
    if kill -0 "$VMAGENT_PID" 2>/dev/null; then
        echo "WARNING: vmagent (PID: $VMAGENT_PID) did not terminate within 10 seconds"
    fi
fi

# Stop the runner-host exporter, if run-vmagent.sh started one.
if [ -f "$VMAGENT_DIR/node_exporter.pid" ]; then
    NODE_EXPORTER_PID=$(cat "$VMAGENT_DIR/node_exporter.pid")
    if [ -n "$NODE_EXPORTER_PID" ] && kill -0 "$NODE_EXPORTER_PID" 2>/dev/null; then
        echo "Stopping node_exporter (PID: $NODE_EXPORTER_PID)..."
        kill "$NODE_EXPORTER_PID" 2>/dev/null || true
    fi
    rm -f "$VMAGENT_DIR/node_exporter.pid"
fi

# Display final log
if [ -n "$VMAGENT_LOG" ] && [ -f "$VMAGENT_LOG" ]; then
    echo "vmagent stopped. Final log:"
    tail -50 "$VMAGENT_LOG" || true
fi

echo "Cleanup complete"

