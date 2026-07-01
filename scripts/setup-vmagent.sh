#!/bin/bash
# Setup and manage vmagent for Victoria Metrics collection
# Usage: setup-vmagent.sh <test_case> <instance_name> <workflow_run_id> <http_port> [commit_hash] [branch]
# The bearer token is read from the VICTORIA_METRICS_BEARER_TOKEN environment
# variable (never passed as an argument, so it can't leak via ps/procfs).

set -euo pipefail

TEST_CASE="${1:-}"
INSTANCE_NAME="${2:-}"
WORKFLOW_RUN_ID="${3:-}"
HTTP_PORT="${4:-8429}"
COMMIT_HASH="${5:-}"
BRANCH="${6:-}"
BEARER_TOKEN="${VICTORIA_METRICS_BEARER_TOKEN:-}"

if [ -z "$TEST_CASE" ] || [ -z "$INSTANCE_NAME" ]; then
    echo "Usage: $0 <test_case> <instance_name> <workflow_run_id> <http_port> [commit_hash] [branch]"
    echo "       (set VICTORIA_METRICS_BEARER_TOKEN in the environment for authenticated writes)"
    exit 1
fi

# Configuration
VMAGENT_VERSION="1.132.0"
VMAGENT_ARCH="amd64"
VMAGENT_DIR="/tmp/vmagent-${TEST_CASE}"
VICTORIA_WRITE_URL="https://victoria-lb.apps.dev.p2p.aws.calimero.network/api/v1/write"
VMAGENT_CONFIG="/tmp/vmagent_scrape_${TEST_CASE}.yml"
VMAGENT_LOG="/tmp/vmagent-${TEST_CASE}.log"

VMAGENT_ASSET="vmutils-linux-${VMAGENT_ARCH}-v${VMAGENT_VERSION}.tar.gz"
VMAGENT_BASE_URL="https://github.com/VictoriaMetrics/VictoriaMetrics/releases/download/v${VMAGENT_VERSION}"
TARBALL="/tmp/vmutils-${TEST_CASE}.tar.gz"
CHECKSUMS="/tmp/vmutils-${TEST_CASE}_checksums.txt"

# Create directories
mkdir -p "$VMAGENT_DIR"

# Download vmagent binary and its upstream-published checksums file
echo "Downloading vmagent v${VMAGENT_VERSION}..."
wget -q -O "$TARBALL" "${VMAGENT_BASE_URL}/${VMAGENT_ASSET}"
wget -q -O "$CHECKSUMS" "${VMAGENT_BASE_URL}/${VMAGENT_ASSET%.tar.gz}_checksums.txt"

if [ ! -f "$TARBALL" ]; then
    echo "ERROR: Failed to download vmagent"
    exit 1
fi

# Verify the archive against the upstream SHA-256 before extracting/executing.
EXPECTED_SHA=$(awk -v f="$VMAGENT_ASSET" '$2 == f {print $1}' "$CHECKSUMS")
if [ -z "$EXPECTED_SHA" ]; then
    echo "ERROR: no checksum for $VMAGENT_ASSET found in checksums file" >&2
    rm -f "$TARBALL" "$CHECKSUMS"
    exit 1
fi
ACTUAL_SHA=$(sha256sum "$TARBALL" | awk '{print $1}')
if [ "$ACTUAL_SHA" != "$EXPECTED_SHA" ]; then
    echo "ERROR: vmagent archive checksum mismatch" >&2
    echo "  expected: $EXPECTED_SHA" >&2
    echo "  actual:   $ACTUAL_SHA" >&2
    rm -f "$TARBALL" "$CHECKSUMS"
    exit 1
fi
echo "vmagent archive checksum verified"

# Extract vmagent-prod binary from tar
tar -xzf "$TARBALL" -C "$VMAGENT_DIR" vmagent-prod

# Rename to vmagent
mv "$VMAGENT_DIR/vmagent-prod" "$VMAGENT_DIR/vmagent"

# Verify binary was extracted
if [ ! -f "$VMAGENT_DIR/vmagent" ]; then
    echo "ERROR: vmagent-prod binary not found in tar archive"
    echo "Archive URL: ${VMAGENT_BASE_URL}/${VMAGENT_ASSET}"
    echo "Archive contents:"
    tar -tzf "$TARBALL" | head -10 || true
    rm -f "$TARBALL" "$CHECKSUMS"
    exit 1
fi

# Clean up downloaded files
rm -f "$TARBALL" "$CHECKSUMS"

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
