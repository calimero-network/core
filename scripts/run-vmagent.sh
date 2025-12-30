#!/bin/bash
# Run vmagent with dynamic process discovery for Victoria Metrics collection
# Usage: run-vmagent.sh <test_case> <instance_name> <workflow_run_id> <commit_hash> <branch> <vmagent_dir> <victoria_url> <auth_enabled> <bearer_token_file> <http_port> <node_pattern>

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

if [ -z "$TEST_CASE" ] || [ -z "$VMAGENT_DIR" ] || [ -z "$VICTORIA_URL" ] || [ -z "$NODE_PATTERN" ]; then
    echo "Usage: $0 <test_case> <instance_name> <workflow_run_id> <commit_hash> <branch> <vmagent_dir> <victoria_url> <auth_enabled> <bearer_token_file> <http_port> <node_pattern>"
    exit 1
fi

VMAGENT_CONFIG="/tmp/vmagent_scrape_${TEST_CASE}.yml"
VMAGENT_LOG="/tmp/vmagent-${TEST_CASE}.log"
VMAGENT_CMD="$VMAGENT_DIR/vmagent"

# Function to generate vmagent scrape config from merod processes
generate_scrape_config() {
    local config_file="$1"
    local test_name="$2"
    local instance_name="$3"
    local run_id="$4"
    local commit_hash="$5"
    local branch="$6"
    local node_pattern="$7"
    
    cat > "$config_file" <<EOF
global:
  scrape_interval: 10s
  external_labels:
    execution_platform: "gha"
    execution_environment: "vm"
    instance_name: "${instance_name}"
    instance_type: "merod"
    test_name: "${test_name}"
    workflow_run: "${run_id}"
    workflow_run_id: "${GITHUB_RUN_ID:-}"
    workflow_run_number: "${GITHUB_RUN_NUMBER:-}"
    commit_sha: "${commit_hash}"
    branch: "${branch}"

scrape_configs:
EOF
    
    # Find all merod processes listening on ports
    # Scan all listening ports in the merod range (2420-2500) and match to merod processes
    local ports_found=0
    while IFS= read -r line; do
        # Extract port and pid from ss output
        # Format: LISTEN 0 4096 0.0.0.0:PORT 0.0.0.0:* users:(("merod",pid=12345,fd=3))
        local port=$(echo "$line" | awk '{print $4}' | cut -d: -f2)
        # Use sed instead of grep -oP for portability
        local pid=$(echo "$line" | sed -n 's/.*pid=\([0-9]*\).*/\1/p' || echo "")
        
        if [ -n "$port" ] && [ -n "$pid" ]; then
            # Verify it's a merod process by checking command line
            local cmdline=$(cat "/proc/${pid}/cmdline" 2>/dev/null | tr '\0' ' ' || echo "")
            if echo "$cmdline" | grep -q "merod.*${node_pattern}"; then
                # Extract node name using sed for portability
                local node_name=$(echo "$cmdline" | sed -n "s/.*\(${node_pattern}-[0-9]*\).*/\1/p" || echo "node-${pid}")
                cat >> "$config_file" <<EOF
  - job_name: "merod-${node_name}"
    scrape_interval: "10s"
    metrics_path: "/metrics"
    static_configs:
      - targets: ["localhost:${port}"]
        labels:
          process_id: "${pid}"
          node_name: "${node_name}"
EOF
                ports_found=$((ports_found + 1))
            fi
        fi
    done < <(ss -tlnp 2>/dev/null | grep LISTEN | grep -E ':(242[0-9]|243[0-9]|244[0-9]|245[0-9]|246[0-9]|247[0-9]|248[0-9]|249[0-9])' || true)
    
    if [ "$ports_found" -eq 0 ]; then
        echo "    # No merod processes found yet, will be discovered dynamically" >> "$config_file"
        echo "    - job_name: 'no-processes'" >> "$config_file"
        echo "      static_configs:" >> "$config_file"
        echo "        - targets: ['placeholder:2428']" >> "$config_file"
    fi
    
    echo "Generated scrape config with $ports_found merod processes" >&2
}

# Generate initial config
generate_scrape_config "$VMAGENT_CONFIG" "$TEST_CASE" "$INSTANCE_NAME" "$WORKFLOW_RUN_ID" "$COMMIT_HASH" "$BRANCH" "$NODE_PATTERN"

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

# Function to update scrape config periodically (for dynamic process discovery)
update_scrape_config_background() {
    local pid="$1"
    local config_file="$2"
    local test_name="$3"
    local instance_name="$4"
    local run_id="$5"
    local commit_hash="$6"
    local branch="$7"
    local node_pattern="$8"
    
    while kill -0 "$pid" 2>/dev/null; do
        sleep 30  # Update every 30 seconds
        if ! generate_scrape_config "$config_file" "$test_name" "$instance_name" "$run_id" "$commit_hash" "$branch" "$node_pattern"; then
            echo "ERROR: Failed to generate scrape config" >&2
        fi
        # Signal vmagent to reload config (SIGHUP)
        if ! kill -HUP "$pid" 2>/dev/null; then
            echo "WARNING: Failed to reload vmagent config" >&2
            break
        fi
    done
}

# Start background task to update config
update_scrape_config_background "$VMAGENT_PID" "$VMAGENT_CONFIG" "$TEST_CASE" "$INSTANCE_NAME" "$WORKFLOW_RUN_ID" "$COMMIT_HASH" "$BRANCH" "$NODE_PATTERN" &
UPDATE_PID=$!

# Export PIDs for cleanup (output to GITHUB_OUTPUT if set, otherwise stdout)
OUTPUT_FILE="${GITHUB_OUTPUT:-/dev/stdout}"
echo "update_pid=$UPDATE_PID" >> "$OUTPUT_FILE"
echo "vmagent_pid=$VMAGENT_PID" >> "$OUTPUT_FILE"

