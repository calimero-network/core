#!/bin/bash
# Run vmagent with static port configuration for Victoria Metrics collection
# Usage: run-vmagent.sh <test_case> <instance_name> <workflow_run_id> <commit_hash> <branch> <vmagent_dir> <victoria_url> <auth_enabled> <bearer_token_file> <http_port> <node_pattern> [node_count] [metrics_port]

set -euo pipefail

TEST_CASE="${1:-}"
INSTANCE_NAME="${2:-}"
WORKFLOW_RUN_ID="${3:-}"
COMMIT_HASH="${4:-}"
BRANCH="${5:-}"
VMAGENT_DIR="${6:-}"
VICTORIA_URL="${7:-}"
AUTH_ENABLED="${8:-false}"
BEARER_TOKEN_FILE="${9:-}"
HTTP_PORT="${10:-8429}"
NODE_PATTERN="${11:-}"  # e.g., "fuzzy-kv-node" or "fuzzy-handlers-node"
NODE_COUNT="${12:-4}"   # Number of nodes (default: 4)
METRICS_PORT="${13:-9528}" # merod's metrics listener inside each node container

if [ -z "$TEST_CASE" ] || [ -z "$VMAGENT_DIR" ] || [ -z "$VICTORIA_URL" ] || [ -z "$NODE_PATTERN" ]; then
    echo "Usage: $0 <test_case> <instance_name> <workflow_run_id> <commit_hash> <branch> <vmagent_dir> <victoria_url> <auth_enabled> <bearer_token_file> <http_port> <node_pattern> [node_count] [metrics_port]"
    exit 1
fi

VMAGENT_CONFIG="/tmp/vmagent_scrape_${TEST_CASE}.yml"
VMAGENT_LOG="/tmp/vmagent-${TEST_CASE}.log"
VMAGENT_CMD="$VMAGENT_DIR/vmagent"
# 10s rather than the 30s production default: a 15-minute run then yields ~90
# samples per series instead of ~30, 1m rate() windows still see several
# points, and a node torn down at the end of a run loses at most 10s.
SCRAPE_INTERVAL="${SCRAPE_INTERVAL:-10s}"
# Runner-host node_exporter (fetched by setup-vmagent.sh), on a port derived
# from vmagent's so suites sharing a host never collide.
NODE_EXPORTER_CMD="$VMAGENT_DIR/node_exporter"
NODE_EXPORTER_PORT=$((HTTP_PORT + 1000))
NODE_EXPORTER_ENABLED="false"

# Function to generate vmagent scrape config, one target per running node container
generate_scrape_config() {
    # Written aside and moved into place, so a reload never reads a half-written file.
    local final_file="$1"
    local config_file="${final_file}.tmp"
    local test_name="$2"
    local instance_name="$3"
    local run_id="$4"
    local commit_hash="$5"
    local branch="$6"
    local node_pattern="$7"
    local node_count="$8"
    local metrics_port="$9"
    
    cat > "$config_file" <<EOF
global:
  scrape_interval: ${SCRAPE_INTERVAL}
  external_labels:
    execution_platform: "gha"
    execution_environment: "vm"
    instance_name: "${instance_name}"
    merod_name: "${instance_name}"
    instance_type: "merod"
    test_name: "${test_name}"
    workflow_run: "${run_id}"
    workflow_run_id: "${GITHUB_RUN_ID:-}"
    workflow_run_number: "${GITHUB_RUN_NUMBER:-}"
    commit_sha: "${commit_hash}"
    branch: "${branch}"

scrape_configs:
EOF
    
    # merod serves /metrics on a listener of its own inside each node's
    # container, so scrape the container's address. A node not up yet joins on
    # the next refresh.
    local targets_found=0
    local node_idx node_name ip
    for node_idx in $(seq 1 "$node_count"); do
        node_name="${node_pattern}-${node_idx}"
        ip=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}' "$node_name" 2>/dev/null | awk '{print $1}' || true)
        [ -n "$ip" ] || continue
        cat >> "$config_file" <<EOF
  - job_name: "merod-${node_name}"
    scrape_interval: "${SCRAPE_INTERVAL}"
    metrics_path: "/metrics"
    static_configs:
      - targets: ["${ip}:${metrics_port}"]
        labels:
          node_name: "${node_name}"
EOF
        targets_found=$((targets_found + 1))
    done
    
    if [ "$NODE_EXPORTER_ENABLED" = "true" ]; then
        cat >> "$config_file" <<EOF
  - job_name: "runner-host"
    scrape_interval: "${SCRAPE_INTERVAL}"
    metrics_path: "/metrics"
    static_configs:
      - targets: ["127.0.0.1:${NODE_EXPORTER_PORT}"]
        labels:
          node_name: "runner"
EOF
    fi

    mv "$config_file" "$final_file"
    echo "Generated scrape config with $targets_found of $node_count node targets (port ${metrics_port})" >&2
}

# Start the runner-host exporter first so the initial config can include it.
if [ -x "$NODE_EXPORTER_CMD" ]; then
    # --web.disable-exporter-metrics: node_exporter would otherwise export
    # its own process_* series (CPU, RSS, start time), which share merod's
    # metric names and would be counted as another node.
    "$NODE_EXPORTER_CMD" --web.listen-address="127.0.0.1:${NODE_EXPORTER_PORT}" \
        --web.disable-exporter-metrics \
        > "$VMAGENT_DIR/node_exporter.log" 2>&1 &
    NODE_EXPORTER_PID=$!
    sleep 1
    if kill -0 "$NODE_EXPORTER_PID" 2>/dev/null; then
        NODE_EXPORTER_ENABLED="true"
        # cleanup-vmagent.sh stops it through this file; its CLI predates the exporter.
        echo "$NODE_EXPORTER_PID" > "$VMAGENT_DIR/node_exporter.pid"
        echo "Started node_exporter on 127.0.0.1:${NODE_EXPORTER_PORT} (PID: $NODE_EXPORTER_PID)"
    else
        echo "WARNING: node_exporter failed to start; runner host metrics disabled" >&2
        tail -5 "$VMAGENT_DIR/node_exporter.log" >&2 || true
    fi
fi

# The labels every scraped series carries, for cleanup-vmagent.sh to stamp on
# the pass/fail result it pushes (vmagent's external_labels cover scrapes only,
# not pushed samples), plus when the run started.
printf '%s\n' "execution_platform=\"gha\",execution_environment=\"vm\",instance_type=\"merod\",instance_name=\"${INSTANCE_NAME}\",merod_name=\"${INSTANCE_NAME}\",test_name=\"${TEST_CASE}\",workflow_run=\"${WORKFLOW_RUN_ID}\",workflow_run_id=\"${GITHUB_RUN_ID:-}\",workflow_run_number=\"${GITHUB_RUN_NUMBER:-}\",commit_sha=\"${COMMIT_HASH}\",branch=\"${BRANCH}\"" \
    > "$VMAGENT_DIR/run_labels"
date +%s > "$VMAGENT_DIR/run_start"

# Generate initial config
generate_scrape_config "$VMAGENT_CONFIG" "$TEST_CASE" "$INSTANCE_NAME" "$WORKFLOW_RUN_ID" "$COMMIT_HASH" "$BRANCH" "$NODE_PATTERN" "$NODE_COUNT" "$METRICS_PORT"

# Validate config file exists and is readable
if [ ! -f "$VMAGENT_CONFIG" ]; then
    echo "ERROR: Failed to generate vmagent config file"
    exit 1
fi

# Build vmagent command
BEARER_FLAG=""
if [ "$AUTH_ENABLED" = "true" ] && [ -n "$BEARER_TOKEN_FILE" ]; then
    BEARER_FLAG="-remoteWrite.bearerTokenFile=$BEARER_TOKEN_FILE"
fi

echo "Starting vmagent..."
echo "VictoriaMetrics URL: $VICTORIA_URL"
echo "Auth enabled: $AUTH_ENABLED"
echo "Instance name: $INSTANCE_NAME"
echo "HTTP listen port: $HTTP_PORT"
echo "Node count: $NODE_COUNT"
echo "Metrics port: $METRICS_PORT"

# Start vmagent in background
$VMAGENT_CMD \
    -promscrape.config="$VMAGENT_CONFIG" \
    -remoteWrite.url="$VICTORIA_URL" \
    -httpListenAddr=:$HTTP_PORT \
    $BEARER_FLAG > "$VMAGENT_LOG" 2>&1 &

VMAGENT_PID=$!
echo "vmagent_pid=$VMAGENT_PID"

# Wait a moment for vmagent to start
sleep 2

# Verify vmagent started successfully
if ! kill -0 $VMAGENT_PID 2>/dev/null; then
    echo "ERROR: vmagent failed to start"
    cat "$VMAGENT_LOG" || true
    exit 1
fi

# Function to update scrape config periodically, adding nodes as their containers start
update_scrape_config_background() {
    local pid="$1"
    local config_file="$2"
    local test_name="$3"
    local instance_name="$4"
    local run_id="$5"
    local commit_hash="$6"
    local branch="$7"
    local node_pattern="$8"
    local node_count="$9"
    local metrics_port="${10}"
    
    while kill -0 "$pid" 2>/dev/null; do
        sleep "$SCRAPE_INTERVAL"
        if ! generate_scrape_config "$config_file" "$test_name" "$instance_name" "$run_id" "$commit_hash" "$branch" "$node_pattern" "$node_count" "$metrics_port"; then
            echo "ERROR: Failed to generate scrape config" >&2
        fi
        # Signal vmagent to reload config (SIGHUP)
        if ! kill -HUP "$pid" 2>/dev/null; then
            echo "WARNING: Failed to reload vmagent config" >&2
            break
        fi
    done
}

# Start background task to update config (adds nodes as they start)
update_scrape_config_background "$VMAGENT_PID" "$VMAGENT_CONFIG" "$TEST_CASE" "$INSTANCE_NAME" "$WORKFLOW_RUN_ID" "$COMMIT_HASH" "$BRANCH" "$NODE_PATTERN" "$NODE_COUNT" "$METRICS_PORT" &
UPDATE_PID=$!

# Export PIDs for cleanup (output to GITHUB_OUTPUT if set, otherwise stdout)
OUTPUT_FILE="${GITHUB_OUTPUT:-/dev/stdout}"
echo "update_pid=$UPDATE_PID" >> "$OUTPUT_FILE"
echo "vmagent_pid=$VMAGENT_PID" >> "$OUTPUT_FILE"
echo "Started vmagent with PID: $VMAGENT_PID"
echo "Started config updater with PID: $UPDATE_PID"

