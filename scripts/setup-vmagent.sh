#!/bin/bash
# Setup and manage vmagent for Victoria Metrics collection
# Usage: setup-vmagent.sh <test_case> <instance_name> <workflow_run_id> <bearer_token> <http_port>

set -euo pipefail

TEST_CASE="${1:-}"
INSTANCE_NAME="${2:-}"
WORKFLOW_RUN_ID="${3:-}"
BEARER_TOKEN="${4:-}"
HTTP_PORT="${5:-8429}"
COMMIT_HASH="${6:-}"
BRANCH="${7:-}"

if [ -z "$TEST_CASE" ] || [ -z "$INSTANCE_NAME" ]; then
    echo "Usage: $0 <test_case> <instance_name> <workflow_run_id> <bearer_token> <http_port> [commit_hash] [branch]"
    exit 1
fi

# Configuration
VMAGENT_VERSION="1.132.0"
VMAGENT_ARCH="amd64"
VMAGENT_DIR="/tmp/vmagent-${TEST_CASE}"
VICTORIA_WRITE_URL="https://victoria-lb.apps.dev.p2p.aws.calimero.network/api/v1/write"
VMAGENT_CONFIG="/tmp/vmagent_scrape_${TEST_CASE}.yml"
VMAGENT_LOG="/tmp/vmagent-${TEST_CASE}.log"

# Create directories
mkdir -p "$VMAGENT_DIR"

# Download vmagent binary
echo "Downloading vmagent v${VMAGENT_VERSION}..."
wget -q -O "/tmp/vmutils-${TEST_CASE}.tar.gz" \
    "https://github.com/VictoriaMetrics/VictoriaMetrics/releases/download/v${VMAGENT_VERSION}/vmutils-linux-${VMAGENT_ARCH}-v${VMAGENT_VERSION}.tar.gz"

if [ ! -f "/tmp/vmutils-${TEST_CASE}.tar.gz" ]; then
    echo "ERROR: Failed to download vmagent"
    exit 1
fi

# Extract vmagent-prod binary from tar
tar -xzf "/tmp/vmutils-${TEST_CASE}.tar.gz" -C "$VMAGENT_DIR" vmagent-prod

# Rename to vmagent
mv "$VMAGENT_DIR/vmagent-prod" "$VMAGENT_DIR/vmagent"

# Verify binary was extracted
if [ ! -f "$VMAGENT_DIR/vmagent" ]; then
    echo "ERROR: vmagent-prod binary not found in tar archive"
    echo "Archive URL: https://github.com/VictoriaMetrics/VictoriaMetrics/releases/download/v${VMAGENT_VERSION}/vmutils-linux-${VMAGENT_ARCH}-v${VMAGENT_VERSION}.tar.gz"
    echo "Archive contents:"
    tar -tzf "/tmp/vmutils-${TEST_CASE}.tar.gz" | head -10 || true
    rm -f "/tmp/vmutils-${TEST_CASE}.tar.gz"
    exit 1
fi

# Clean up tar file
rm -f "/tmp/vmutils-${TEST_CASE}.tar.gz"

chmod +x "$VMAGENT_DIR/vmagent"

# Verify binary exists
if [ ! -f "$VMAGENT_DIR/vmagent" ]; then
    echo "ERROR: Failed to extract vmagent binary"
    exit 1
fi

# Save bearer token to file
AUTH_ENABLED="false"
BEARER_TOKEN_FILE=""
if [ -n "$BEARER_TOKEN" ]; then
    echo "$BEARER_TOKEN" > "$VMAGENT_DIR/bearer_token"
    chmod 600 "$VMAGENT_DIR/bearer_token"
    AUTH_ENABLED="true"
    BEARER_TOKEN_FILE="$VMAGENT_DIR/bearer_token"
else
    echo "Warning: Bearer token not provided, metrics will be sent without auth"
fi

# Export variables for use in workflow (output to GITHUB_OUTPUT if set, otherwise stdout)
OUTPUT_FILE="${GITHUB_OUTPUT:-/dev/stdout}"
{
    echo "vmagent_dir=$VMAGENT_DIR"
    echo "victoria_url=$VICTORIA_WRITE_URL"
    echo "auth_enabled=$AUTH_ENABLED"
    echo "bearer_token_file=$BEARER_TOKEN_FILE"
    echo "vmagent_config=$VMAGENT_CONFIG"
    echo "vmagent_log=$VMAGENT_LOG"
} >> "$OUTPUT_FILE"

