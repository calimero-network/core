#!/bin/bash
# Cleanup vmagent and background processes
# Usage: cleanup-vmagent.sh <test_case> <update_pid> <vmagent_pid> <vmagent_config> <vmagent_log>

set -euo pipefail

TEST_CASE="${1:-}"
UPDATE_PID="${2:-}"
VMAGENT_PID="${3:-}"
VMAGENT_CONFIG="${4:-}"
VMAGENT_LOG="${5:-}"

if [ -z "$TEST_CASE" ]; then
    echo "Usage: $0 <test_case> <update_pid> <vmagent_pid> <vmagent_config> <vmagent_log>"
    exit 1
fi

echo "Cleaning up vmagent for test case: $TEST_CASE"

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
    wait "$UPDATE_PID" 2>/dev/null || true
fi

# Stop vmagent
if [ -n "$VMAGENT_PID" ] && kill -0 "$VMAGENT_PID" 2>/dev/null; then
    echo "Stopping vmagent (PID: $VMAGENT_PID)..."
    kill "$VMAGENT_PID" 2>/dev/null || true
    wait "$VMAGENT_PID" 2>/dev/null || true
fi

# Display final log
if [ -n "$VMAGENT_LOG" ] && [ -f "$VMAGENT_LOG" ]; then
    echo "vmagent stopped. Final log:"
    tail -50 "$VMAGENT_LOG" || true
fi

echo "Cleanup complete"

