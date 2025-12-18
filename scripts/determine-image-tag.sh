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
    echo "Checking if Rust crates changed in PR..."
    
    CHANGED_FILES=$(gh api "repos/${REPO}/pulls/${PR_NUMBER}/files" --jq '.[].filename' 2>/dev/null || echo "")
    CRATES_CHANGED=$(echo "$CHANGED_FILES" | \
        grep -E '^(Cargo\.toml|Cargo\.lock|crates/|\.github/workflows/release\.yml)' || true)
    
    if [ -n "$CRATES_CHANGED" ]; then
        echo "Rust crates changed - release workflow will build/rebuild pr-${PR_NUMBER} image"
        echo "Waiting for release workflow to complete..."
        
        MAX_WAIT=1200
        WAIT_INTERVAL=10
        ELAPSED=0
        
        while [ $ELAPSED -lt $MAX_WAIT ]; do
            RUNS=$(gh api "repos/${REPO}/actions/workflows/release.yml/runs?head_branch=${HEAD_BRANCH}" --jq '.workflow_runs[0] // {"status":"unknown"}' 2>/dev/null || echo '{"status":"unknown"}')
            STATUS=$(echo "$RUNS" | jq -r '.status // "unknown"')
            CONCLUSION=$(echo "$RUNS" | jq -r '.conclusion // "unknown"')
            
            if [ "$STATUS" = "completed" ]; then
                if [ "$CONCLUSION" = "success" ]; then
                    echo "Release workflow completed successfully after ${ELAPSED} seconds"
                    TAG="pr-${PR_NUMBER}"
                else
                    echo "Release workflow completed with conclusion: ${CONCLUSION}"
                    echo "Checking if pr-${PR_NUMBER} image exists from previous successful build..."
                    PR_TAG="pr-${PR_NUMBER}"
                    IMAGE_EXISTS="false"
                    
                    if command -v docker >/dev/null 2>&1; then
                        echo "${GH_TOKEN}" | docker login ghcr.io -u "${REPO_OWNER}" --password-stdin >/dev/null 2>&1
                        if docker manifest inspect "ghcr.io/calimero-network/merod:${PR_TAG}" >/dev/null 2>&1; then
                            IMAGE_EXISTS="true"
                            echo "Image pr-${PR_NUMBER} exists from previous build - using it"
                            TAG="pr-${PR_NUMBER}"
                        else
                            echo "Image pr-${PR_NUMBER} not found"
                        fi
                    fi
                    
                    if [ "$IMAGE_EXISTS" != "true" ]; then
                        echo "Using edge tag as fallback"
                        TAG="edge"
                    fi
                fi
                break
            fi
            
            if [ "$STATUS" = "queued" ] || [ "$STATUS" = "in_progress" ]; then
                echo "Release workflow ${STATUS}... waiting (${ELAPSED}s/${MAX_WAIT}s)"
            elif [ "$STATUS" = "unknown" ]; then
                echo "Release workflow not found yet... waiting (${ELAPSED}s/${MAX_WAIT}s)"
            else
                echo "Release workflow status: ${STATUS}... waiting (${ELAPSED}s/${MAX_WAIT}s)"
            fi
            
            sleep $WAIT_INTERVAL
            ELAPSED=$((ELAPSED + WAIT_INTERVAL))
        done
        
        if [ $ELAPSED -ge $MAX_WAIT ]; then
            echo "Timeout waiting for release workflow (waited ${MAX_WAIT}s)"
            echo "Using pr-${PR_NUMBER} tag (workflow may complete later)"
            TAG="pr-${PR_NUMBER}"
        fi
    else
        echo "No Rust crates changed in this commit"
        echo "Checking if pr-${PR_NUMBER} image exists (from previous commits)..."
        
        PR_TAG="pr-${PR_NUMBER}"
        IMAGE_EXISTS="false"
        
        if command -v docker >/dev/null 2>&1; then
            echo "${GH_TOKEN}" | docker login ghcr.io -u "${REPO_OWNER}" --password-stdin >/dev/null 2>&1
            if docker manifest inspect "ghcr.io/calimero-network/merod:${PR_TAG}" >/dev/null 2>&1; then
                IMAGE_EXISTS="true"
                echo "Image pr-${PR_NUMBER} exists - using it"
                TAG="pr-${PR_NUMBER}"
            else
                echo "Image pr-${PR_NUMBER} not found"
            fi
        fi
        
        if [ "$IMAGE_EXISTS" != "true" ]; then
            echo "Using edge tag"
            TAG="edge"
        fi
    fi
else
    TAG="edge"
fi

echo "tag=${TAG}" >> "$GITHUB_OUTPUT"
echo "Using Docker image: ghcr.io/calimero-network/merod:${TAG}"

