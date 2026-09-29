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
# SHA-256 of vmutils-linux-${VMAGENT_ARCH}-v${VMAGENT_VERSION}.tar.gz, pinned
# here in-repo (taken from the upstream _checksums.txt) so the trust root is
# this reviewed file rather than a checksums file fetched from the same host as
# the archive. Refresh it whenever VMAGENT_VERSION or VMAGENT_ARCH changes.
VMAGENT_SHA256="fd3eaa294050fc849931e8947212c736825a7a49e5509ee4555e288750861fc8"
VMAGENT_DIR="/tmp/vmagent-${TEST_CASE}"
VICTORIA_WRITE_URL="https://victoria-lb.apps.dev.p2p.aws.calimero.network/api/v1/write"
VMAGENT_CONFIG="/tmp/vmagent_scrape_${TEST_CASE}.yml"
VMAGENT_LOG="/tmp/vmagent-${TEST_CASE}.log"

VMAGENT_ASSET="vmutils-linux-${VMAGENT_ARCH}-v${VMAGENT_VERSION}.tar.gz"
VMAGENT_BASE_URL="https://github.com/VictoriaMetrics/VictoriaMetrics/releases/download/v${VMAGENT_VERSION}"
TARBALL="/tmp/vmutils-${TEST_CASE}.tar.gz"

# Create directories
mkdir -p "$VMAGENT_DIR"

# Download vmagent archive. wget exits non-zero on HTTP errors and `set -e`
# aborts the script, so a failed download never reaches the checks below.
echo "Downloading vmagent v${VMAGENT_VERSION}..."
wget -q -O "$TARBALL" "${VMAGENT_BASE_URL}/${VMAGENT_ASSET}"

# Verify against the pinned SHA-256 before extracting/executing anything.
ACTUAL_SHA=$(sha256sum "$TARBALL" | awk '{print $1}')
if [ "$ACTUAL_SHA" != "$VMAGENT_SHA256" ]; then
    echo "ERROR: vmagent archive checksum mismatch" >&2
    echo "  expected: $VMAGENT_SHA256" >&2
    echo "  actual:   $ACTUAL_SHA" >&2
    rm -f "$TARBALL"
    exit 1
fi
echo "vmagent archive checksum verified"

# Extract vmagent-prod. Handle failure explicitly (rather than letting `set -e`
# abort) so the archive contents can be printed for diagnosis while the tarball
# still exists.
if ! tar -xzf "$TARBALL" -C "$VMAGENT_DIR" vmagent-prod; then
    echo "ERROR: vmagent-prod not found in $VMAGENT_ASSET" >&2
    echo "Archive contents:" >&2
    tar -tzf "$TARBALL" | head -10 >&2 || true
    rm -f "$TARBALL"
    exit 1
fi

# Rename to vmagent and clean up the archive
mv "$VMAGENT_DIR/vmagent-prod" "$VMAGENT_DIR/vmagent"
rm -f "$TARBALL"

chmod +x "$VMAGENT_DIR/vmagent"

# node_exporter reports the runner host itself (CPU, steal, load, memory,
# free disk) so a slow or failing run can be told apart from a saturated or
# full runner. Pinned and verified the same way as vmagent. Best-effort: a
# failed download only loses the host series, never the merod metrics.
NODE_EXPORTER_VERSION="1.12.1"
# SHA-256 of node_exporter-${NODE_EXPORTER_VERSION}.linux-${VMAGENT_ARCH}.tar.gz,
# from the upstream sha256sums.txt. Refresh it with the version.
NODE_EXPORTER_SHA256="b51d8a76aa2a9156a55d501aca6276fae09e262259a5e4e831d2c2222f084e63"
NODE_EXPORTER_NAME="node_exporter-${NODE_EXPORTER_VERSION}.linux-${VMAGENT_ARCH}"
NODE_EXPORTER_TARBALL="/tmp/${NODE_EXPORTER_NAME}-${TEST_CASE}.tar.gz"

echo "Downloading node_exporter v${NODE_EXPORTER_VERSION}..."
if wget -q -O "$NODE_EXPORTER_TARBALL" \
    "https://github.com/prometheus/node_exporter/releases/download/v${NODE_EXPORTER_VERSION}/${NODE_EXPORTER_NAME}.tar.gz"; then
    ACTUAL_SHA=$(sha256sum "$NODE_EXPORTER_TARBALL" | awk '{print $1}')
    if [ "$ACTUAL_SHA" = "$NODE_EXPORTER_SHA256" ] &&
        tar -xzf "$NODE_EXPORTER_TARBALL" -C "$VMAGENT_DIR" --strip-components=1 \
            "${NODE_EXPORTER_NAME}/node_exporter"; then
        chmod +x "$VMAGENT_DIR/node_exporter"
        echo "node_exporter checksum verified"
    else
        echo "WARNING: node_exporter checksum mismatch or bad archive; runner host metrics disabled" >&2
        rm -f "$VMAGENT_DIR/node_exporter"
    fi
else
    echo "WARNING: node_exporter download failed; runner host metrics disabled" >&2
fi
rm -f "$NODE_EXPORTER_TARBALL"

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
