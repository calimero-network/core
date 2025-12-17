#!/bin/bash
set -euo pipefail

# Determine Docker image tag for GitHub Actions workflows
# Usage: determine-image-tag.sh <event_name> <pr_number> <repo_owner> <repo> <head_branch> <gh_token>
# repo is in format "owner/repo"
# Outputs: tag=<tag> to GITHUB_OUTPUT

EVENT_NAME="$1"
PR_NUMBER="$2"
REPO_OWNER="$3"
REPO="$4"
HEAD_BRANCH="$5"
GH_TOKEN="$6"

if [ "$EVENT_NAME" == "pull_request" ]; then
    # Check if PR image exists, waiting for release workflow if needed
    # PR images are always under calimero-network (only built for non-fork PRs)
    PR_TAG="pr-${PR_NUMBER}"
    IMAGE_URL="https://ghcr.io/v2/calimero-network/merod/manifests/${PR_TAG}"
    MAX_WAIT=600
    WAIT_INTERVAL=10
    ELAPSED=0
    IMAGE_EXISTS="false"
    WORKFLOW_COMPLETED="false"
    
    echo "Checking for PR image: ${IMAGE_URL}"
    
    while [ $ELAPSED -lt $MAX_WAIT ]; do
        # Check if image exists using GHCR API
        # GHCR uses Bearer authentication with GitHub tokens
        HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
            -H "Authorization: Bearer ${GH_TOKEN}" \
            -H "Accept: application/vnd.docker.distribution.manifest.v2+json" \
            "${IMAGE_URL}" 2>/dev/null || echo "404")
        
        if [ "$HTTP_CODE" = "200" ]; then
            IMAGE_EXISTS="true"
            echo "Found PR image after ${ELAPSED} seconds"
            break
        fi
        
        # Check release workflow status
        RUNS=$(gh api repos/${REPO}/actions/workflows/release.yml/runs?head_branch=${HEAD_BRANCH} --jq '.workflow_runs[0] // {"status":"unknown"}' 2>/dev/null || echo '{"status":"unknown"}')
        STATUS=$(echo "$RUNS" | jq -r '.status // "unknown"')
        CONCLUSION=$(echo "$RUNS" | jq -r '.conclusion // "unknown"')
        
        if [ "$STATUS" = "completed" ]; then
            WORKFLOW_COMPLETED="true"
            echo "Release workflow completed with conclusion: ${CONCLUSION}"
            # Don't break immediately - do one more image check first
            sleep 2
            HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
                -H "Authorization: Bearer ${GH_TOKEN}" \
                -H "Accept: application/vnd.docker.distribution.manifest.v2+json" \
                "${IMAGE_URL}" 2>/dev/null || echo "404")
            if [ "$HTTP_CODE" = "200" ]; then
                IMAGE_EXISTS="true"
                echo "Found PR image after workflow completed"
            fi
            break
        fi
        
        sleep $WAIT_INTERVAL
        ELAPSED=$((ELAPSED + WAIT_INTERVAL))
    done
    
    # Final check if we didn't find it during the wait loop
    if [ "$IMAGE_EXISTS" = "false" ]; then
        echo "Final check for PR image..."
        HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
            -H "Authorization: Bearer ${GH_TOKEN}" \
            -H "Accept: application/vnd.docker.distribution.manifest.v2+json" \
            "${IMAGE_URL}" 2>/dev/null || echo "404")
        if [ "$HTTP_CODE" = "200" ]; then
            IMAGE_EXISTS="true"
            echo "Found PR image in final check"
        else
            echo "PR image not found (HTTP ${HTTP_CODE})"
        fi
    fi
    
    if [ "$IMAGE_EXISTS" = "true" ]; then
        TAG="pr-$PR_NUMBER"
    else
        TAG="edge"
    fi
else
    TAG="edge"
fi

echo "tag=${TAG}" >> "$GITHUB_OUTPUT"
echo "Using Docker image: ghcr.io/calimero-network/merod:${TAG}"

