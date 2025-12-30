#!/bin/bash
# Run vmagent with static port configuration for Victoria Metrics collection
# Usage: run-vmagent.sh <test_case> <instance_name> <workflow_run_id> <commit_hash> <branch> <vmagent_dir> <victoria_url> <auth_enabled> <bearer_token_file> <http_port> <node_pattern> [node_count] [base_port] [port_increment]

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
BASE_PORT="${13:-3001}" # Starting port (default: 3001 for e2e_mode RPC ports)
PORT_INCREMENT="${14:-2}" # Port increment between nodes (default: 2 for e2e_mode)

if [ -z "$TEST_CASE" ] || [ -z "$VMAGENT_DIR" ] || [ -z "$VICTORIA_URL" ] || [ -z "$NODE_PATTERN" ]; then
    echo "Usage: $0 <test_case> <instance_name> <workflow_run_id> <commit_hash> <branch> <vmagent_dir> <victoria_url> <auth_enabled> <bearer_token_file> <http_port> <node_pattern> [node_count] [base_port] [port_increment]"
    exit 1
fi

VMAGENT_CONFIG="/tmp/vmagent_scrape_${TEST_CASE}.yml"
VMAGENT_LOG="/tmp/vmagent-${TEST_CASE}.log"
VMAGENT_CMD="$VMAGENT_DIR/vmagent"

# Function to generate vmagent scrape config using static port configuration
# Uses predictable ports starting from base_port, incrementing by port_increment for each node
generate_scrape_config() {
    local config_file="$1"
    local test_name="$2"
    local instance_name="$3"
    local run_id="$4"
    local commit_hash="$5"
    local branch="$6"
    local node_pattern="$7"
    local node_count="$8"
    local base_port="$9"
    local port_increment="${10:-2}"
    
    cat > "$config_file" <<EOF
global:
  scrape_interval: 5s
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
    
    # Generate static targets for predictable ports
    # Ports start from base_port and increment by port_increment for each node
    # e.g., base_port=3001, port_increment=2: 3001, 3003, 3005, 3007
    local ports_found=0
    local port
    local node_idx
    
    for node_idx in $(seq 1 "$node_count"); do
        port=$((base_port + (node_idx - 1) * port_increment))
        local node_name="${node_pattern}-${node_idx}"
        
        # Try to find process PID for better labeling (optional, won't fail if not found)
        local pid=""
        
        # Attempt to find PID listening on this port (may fail in CI, that's OK)
        if command -v ss >/dev/null 2>&1; then
            local ss_line=$(ss -tlnp 2>/dev/null | grep ":${port}" | grep LISTEN | head -1 || true)
            if [ -n "$ss_line" ]; then
                pid=$(echo "$ss_line" | sed -n 's/.*pid=\([0-9]*\).*/\1/p' || echo "")
                if [ -n "$pid" ] && [ -d "/proc/${pid}" ]; then
                    # Verify it matches our node pattern
                    local cmdline=$(cat "/proc/${pid}/cmdline" 2>/dev/null | tr '\0' ' ' || echo "")
                    if ! echo "$cmdline" | grep -q "${node_pattern}"; then
                        # PID doesn't match pattern, clear it
                        pid=""
                    fi
                else
                    pid=""
                fi
            fi
        fi
        
        # Write config with or without process_id label
        if [ -n "$pid" ]; then
            cat >> "$config_file" <<EOF
  - job_name: "merod-${node_name}"
    scrape_interval: "5s"
    metrics_path: "/metrics"
    static_configs:
      - targets: ["localhost:${port}"]
        labels:
          node_name: "${node_name}"
          process_id: "${pid}"
EOF
        else
            cat >> "$config_file" <<EOF
  - job_name: "merod-${node_name}"
    scrape_interval: "5s"
    metrics_path: "/metrics"
    static_configs:
      - targets: ["localhost:${port}"]
        labels:
          node_name: "${node_name}"
EOF
        fi
        ports_found=$((ports_found + 1))
    done
    
    local last_port=$((base_port + (node_count - 1) * port_increment))
    echo "Generated scrape config with $ports_found static targets (ports ${base_port}-${last_port}, increment ${port_increment})" >&2
}

# Generate initial config
generate_scrape_config "$VMAGENT_CONFIG" "$TEST_CASE" "$INSTANCE_NAME" "$WORKFLOW_RUN_ID" "$COMMIT_HASH" "$BRANCH" "$NODE_PATTERN" "$NODE_COUNT" "$BASE_PORT" "$PORT_INCREMENT"

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
echo "Base port: $BASE_PORT"
echo "Port increment: $PORT_INCREMENT"
LAST_PORT=$((BASE_PORT + (NODE_COUNT - 1) * PORT_INCREMENT))
echo "Ports: ${BASE_PORT}-${LAST_PORT} (increment ${PORT_INCREMENT})"

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

# Function to update scrape config periodically (to refresh process info labels)
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
    local base_port="${10}"
    local port_increment="${11:-2}"
    
    while kill -0 "$pid" 2>/dev/null; do
        sleep 30  # Update every 30 seconds to refresh process info
        if ! generate_scrape_config "$config_file" "$test_name" "$instance_name" "$run_id" "$commit_hash" "$branch" "$node_pattern" "$node_count" "$base_port" "$port_increment"; then
            echo "ERROR: Failed to generate scrape config" >&2
        fi
        # Signal vmagent to reload config (SIGHUP)
        if ! kill -HUP "$pid" 2>/dev/null; then
            echo "WARNING: Failed to reload vmagent config" >&2
            break
        fi
    done
}

# Start background task to update config (refreshes process info labels)
update_scrape_config_background "$VMAGENT_PID" "$VMAGENT_CONFIG" "$TEST_CASE" "$INSTANCE_NAME" "$WORKFLOW_RUN_ID" "$COMMIT_HASH" "$BRANCH" "$NODE_PATTERN" "$NODE_COUNT" "$BASE_PORT" "$PORT_INCREMENT" &
UPDATE_PID=$!

# Export PIDs for cleanup (output to GITHUB_OUTPUT if set, otherwise stdout)
OUTPUT_FILE="${GITHUB_OUTPUT:-/dev/stdout}"
echo "update_pid=$UPDATE_PID" >> "$OUTPUT_FILE"
echo "vmagent_pid=$VMAGENT_PID" >> "$OUTPUT_FILE"
echo "Started vmagent with PID: $VMAGENT_PID"
echo "Started config updater with PID: $UPDATE_PID"

