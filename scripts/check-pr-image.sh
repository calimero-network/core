#!/bin/bash
set -euo pipefail

# Check if PR Docker image exists, waiting for release workflow if needed
# Usage: check-pr-image.sh <pr_number> <repo_owner> <repo> <head_branch> <gh_token>
# repo is in format "owner/repo"

PR_NUMBER="$1"
REPO_OWNER="$2"
REPO="$3"
HEAD_BRANCH="$4"
GH_TOKEN="$5"

PR_TAG="pr-${PR_NUMBER}"
IMAGE_URL="https://ghcr.io/v2/${REPO_OWNER}/merod/manifests/${PR_TAG}"
MAX_WAIT=600
WAIT_INTERVAL=10
ELAPSED=0

while [ $ELAPSED -lt $MAX_WAIT ]; do
    HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "Authorization: Bearer ${GH_TOKEN}" "${IMAGE_URL}" 2>/dev/null || echo "404")
    if [ "$HTTP_CODE" = "200" ]; then
        echo "true"
        exit 0
    fi
    
    RUNS=$(gh api repos/${REPO}/actions/workflows/release.yml/runs?head_branch=${HEAD_BRANCH} --jq '.workflow_runs[0] // {"status":"unknown"}' 2>/dev/null || echo '{"status":"unknown"}')
    STATUS=$(echo "$RUNS" | jq -r '.status // "unknown"')
    
    if [ "$STATUS" = "completed" ]; then
        break
    fi
    
    sleep $WAIT_INTERVAL
    ELAPSED=$((ELAPSED + WAIT_INTERVAL))
done

HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "Authorization: Bearer ${GH_TOKEN}" "${IMAGE_URL}" 2>/dev/null || echo "404")
if [ "$HTTP_CODE" = "200" ]; then
    echo "true"
else
    echo "false"
fi

