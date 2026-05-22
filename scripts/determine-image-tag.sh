#!/bin/bash
set -euo pipefail

# Determine Docker image tag for GitHub Actions workflows
# Usage: determine-image-tag.sh <event_name> <pr_number> <repo_owner> <repo> <head_branch> <gh_token> [--profiling]
# repo is in format "owner/repo"
# Options:
#   --profiling  Use profiling image variant (adds -profiling suffix to tags)
# Outputs: tag=<tag> to GITHUB_OUTPUT

EVENT_NAME="$1"
PR_NUMBER="$2"
REPO_OWNER="$3"
REPO="$4"
HEAD_BRANCH="$5"
GH_TOKEN="$6"
PROFILING_MODE="${7:-}"

# Determine suffix based on mode
if [ "$PROFILING_MODE" = "--profiling" ]; then
    TAG_SUFFIX="-profiling"
    IMAGE_TYPE="profiling"
    echo "Running in profiling mode - will use *-profiling image tags"
else
    TAG_SUFFIX=""
    IMAGE_TYPE="standard"
fi

if [ "$EVENT_NAME" == "pull_request" ]; then
    echo "Checking if Rust crates or release-triggering files changed in PR..."
    
    CHANGED_FILES=$(gh api "repos/${REPO}/pulls/${PR_NUMBER}/files" --jq '.[].filename' 2>/dev/null || echo "")
    # Match release.yml's `pull_request.paths` trigger so the wait loop
    # below knows when release.yml is expected to (re)publish a
    # `pr-<N>-profiling` image we should pull. `scripts/profiling/` is
    # listed because release.yml's profiling-image build now also
    # fires on PRs that touch those files (see prepare.profiling_paths_changed).
    CRATES_CHANGED=$(echo "$CHANGED_FILES" | \
        grep -E '^(Cargo\.toml|Cargo\.lock|crates/|\.github/workflows/release\.yml|\.github/workflows/deps/|\.github/actions/|scripts/profiling/)' || true)
    
    if [ -n "$CRATES_CHANGED" ]; then
        echo "Rust crates or release-triggering files changed - release workflow will build/rebuild pr-${PR_NUMBER} image"
        echo "Waiting for release workflow to complete..."
        
        if [ "$PROFILING_MODE" = "--profiling" ]; then
            MAX_WAIT=1800  # 30 minutes for profiling containers
            echo "Profiling mode: extended wait time to 30 minutes (profiling containers take longer to build)"
        else
            MAX_WAIT=1200  # 20 minutes for standard containers
        fi
        WAIT_INTERVAL=10
        ELAPSED=0
        
        # Get PR head SHA for more accurate workflow run filtering
        PR_HEAD_SHA=$(gh api "repos/${REPO}/pulls/${PR_NUMBER}" --jq '.head.sha' 2>/dev/null || echo "")
        
        while [ $ELAPSED -lt $MAX_WAIT ]; do
            # Filter workflow runs by PR number first (most accurate)
            # Get all runs and filter by PR number in pull_requests array
            RUNS='{"status":"unknown"}'
            
            if [ -n "$PR_HEAD_SHA" ]; then
                # Get runs for this commit SHA and filter by PR number
                ALL_RUNS=$(gh api "repos/${REPO}/actions/workflows/release.yml/runs?head_sha=${PR_HEAD_SHA}" --jq '.workflow_runs' 2>/dev/null || echo "[]")
                RUNS=$(echo "$ALL_RUNS" | jq --arg pr_num "$PR_NUMBER" '[.[] | select(.pull_requests[]?.number == ($pr_num | tonumber))] | .[0] // {"status":"unknown"}' 2>/dev/null || echo '{"status":"unknown"}')
            fi
            
            # Fallback: If no PR-filtered run found, try head SHA
            if [ "$(echo "$RUNS" | jq -r '.status // "unknown"')" = "unknown" ] && [ -n "$PR_HEAD_SHA" ]; then
                RUNS=$(gh api "repos/${REPO}/actions/workflows/release.yml/runs?head_sha=${PR_HEAD_SHA}" --jq '.workflow_runs[0] // {"status":"unknown"}' 2>/dev/null || echo '{"status":"unknown"}')
            fi
            
            # Final fallback: branch name
            if [ "$(echo "$RUNS" | jq -r '.status // "unknown"')" = "unknown" ]; then
                RUNS=$(gh api "repos/${REPO}/actions/workflows/release.yml/runs?head_branch=${HEAD_BRANCH}" --jq '.workflow_runs[0] // {"status":"unknown"}' 2>/dev/null || echo '{"status":"unknown"}')
            fi
            
            STATUS=$(echo "$RUNS" | jq -r '.status // "unknown"')
            CONCLUSION=$(echo "$RUNS" | jq -r '.conclusion // "unknown"')
            
            if [ "$STATUS" = "completed" ]; then
                if [ "$CONCLUSION" = "success" ]; then
                    echo "Release workflow completed successfully after ${ELAPSED} seconds"
                    TAG="pr-${PR_NUMBER}${TAG_SUFFIX}"
                else
                    echo "Release workflow completed with conclusion: ${CONCLUSION}"
                    echo "Checking if pr-${PR_NUMBER}${TAG_SUFFIX} image exists from previous successful build..."
                    PR_TAG="pr-${PR_NUMBER}${TAG_SUFFIX}"
                    IMAGE_EXISTS="false"
                    
                    if command -v docker >/dev/null 2>&1; then
                        echo "${GH_TOKEN}" | docker login ghcr.io -u "${REPO_OWNER}" --password-stdin >/dev/null 2>&1
                        if docker manifest inspect "ghcr.io/calimero-network/merod:${PR_TAG}" >/dev/null 2>&1; then
                            IMAGE_EXISTS="true"
                            echo "Image ${PR_TAG} exists from previous build - using it"
                            TAG="pr-${PR_NUMBER}${TAG_SUFFIX}"
                        else
                            echo "Image ${PR_TAG} not found"
                        fi
                    fi
                    
                    if [ "$IMAGE_EXISTS" != "true" ]; then
                        echo "Using edge${TAG_SUFFIX} tag as fallback"
                        TAG="edge${TAG_SUFFIX}"
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
            echo "Checking if pr-${PR_NUMBER}${TAG_SUFFIX} image exists before using it..."
            PR_TAG="pr-${PR_NUMBER}${TAG_SUFFIX}"
            IMAGE_EXISTS="false"
            
            if command -v docker >/dev/null 2>&1; then
                echo "${GH_TOKEN}" | docker login ghcr.io -u "${REPO_OWNER}" --password-stdin >/dev/null 2>&1
                if docker manifest inspect "ghcr.io/calimero-network/merod:${PR_TAG}" >/dev/null 2>&1; then
                    IMAGE_EXISTS="true"
                    echo "Image ${PR_TAG} exists - using it (workflow may complete later)"
                    TAG="pr-${PR_NUMBER}${TAG_SUFFIX}"
                else
                    echo "Image ${PR_TAG} not found"
                fi
            fi
            
            if [ "$IMAGE_EXISTS" != "true" ]; then
                echo "Using edge${TAG_SUFFIX} tag as fallback"
                TAG="edge${TAG_SUFFIX}"
            fi
        fi
    else
        echo "No Rust crates or release-triggering files changed in this commit"
        echo "Checking if pr-${PR_NUMBER}${TAG_SUFFIX} image exists (from previous commits)..."
        
        PR_TAG="pr-${PR_NUMBER}${TAG_SUFFIX}"
        IMAGE_EXISTS="false"
        
        if command -v docker >/dev/null 2>&1; then
            echo "${GH_TOKEN}" | docker login ghcr.io -u "${REPO_OWNER}" --password-stdin >/dev/null 2>&1
            if docker manifest inspect "ghcr.io/calimero-network/merod:${PR_TAG}" >/dev/null 2>&1; then
                IMAGE_EXISTS="true"
                echo "Image ${PR_TAG} exists - using it"
                TAG="pr-${PR_NUMBER}${TAG_SUFFIX}"
            else
                echo "Image ${PR_TAG} not found"
            fi
        fi
        
        if [ "$IMAGE_EXISTS" != "true" ]; then
            echo "Using edge${TAG_SUFFIX} tag"
            TAG="edge${TAG_SUFFIX}"
        fi
    fi
elif [ "$EVENT_NAME" = "push" ] && [ "$HEAD_BRANCH" = "master" ]; then
    # Master push: release.yml is triggered in parallel with this caller
    # workflow (e.g. fuzzy-load-test.yml), and the moving `:edge${TAG_SUFFIX}`
    # tag in ghcr only gets re-pointed at the just-merged binary once
    # release.yml's `Release Merod Profiling container` (or its non-profiling
    # twin) finishes. Until then, `:edge${TAG_SUFFIX}` still points at the
    # PREVIOUS master commit's binary.
    #
    # Without this wait, every master push gets one guaranteed false-positive
    # cycle on every workflow that pulls `:edge${TAG_SUFFIX}`, because the
    # test pulls the prior binary and exercises whatever bugs that prior
    # commit had. Mirror the PR-branch wait loop so the test only proceeds
    # once the new image is actually live.
    HEAD_SHA="${GITHUB_SHA:-}"

    if [ -z "$HEAD_SHA" ]; then
        # GITHUB_SHA is set automatically in every step inside GitHub
        # Actions, so this branch fires only on local/manual invocation
        # (e.g. running the script by hand for testing).
        echo "GITHUB_SHA not set - assuming local invocation, skipping release.yml wait"
        echo "Falling back to :edge${TAG_SUFFIX} as-is"
        TAG="edge${TAG_SUFFIX}"
    else
        # release.yml only fires on master pushes that touch its `push.paths`
        # filter (`Cargo.{toml,lock}`, `crates/**`, `.github/workflows/release.yml`,
        # `.github/workflows/deps/**`, `.github/actions/**`, `scripts/profiling/**`).
        # A master push that only touches `apps/**`, `docs/**`, other workflows,
        # `README.md`, etc. won't trigger release.yml at all — in which case
        # `:edge${TAG_SUFFIX}` doesn't change either (no Rust code rebuilt),
        # so waiting for a run that will never appear would just stall the
        # consumer for the full timeout. Skip the wait in that case, matching
        # the PR-branch's `CRATES_CHANGED` early-exit.
        CHANGED_FILES=$(gh api "repos/${REPO}/commits/${HEAD_SHA}" \
            --jq '.files[].filename' 2>/dev/null || echo "")
        RELEASE_TRIGGERING_CHANGED=$(echo "$CHANGED_FILES" | \
            grep -E '^(Cargo\.toml|Cargo\.lock|crates/|\.github/workflows/release\.yml|\.github/workflows/deps/|\.github/actions/|scripts/profiling/)' || true)

        if [ -z "$RELEASE_TRIGGERING_CHANGED" ]; then
            echo "Master push touches no release.yml trigger paths - release.yml will not fire on ${HEAD_SHA}"
            echo "Resolving :edge${TAG_SUFFIX} immediately (no new image expected)"
            TAG="edge${TAG_SUFFIX}"
        else
            echo "Master push touches release.yml trigger paths - waiting for release.yml on ${HEAD_SHA} to republish :edge${TAG_SUFFIX}"

            if [ "$PROFILING_MODE" = "--profiling" ]; then
                MAX_WAIT=1800  # 30 minutes - profiling container build takes longer
            else
                MAX_WAIT=1200  # 20 minutes - same budget as the PR-branch path
            fi
            WAIT_INTERVAL=10
            ELAPSED=0

            while [ $ELAPSED -lt $MAX_WAIT ]; do
                # Filter release.yml runs by the exact merge commit SHA. On
                # master push, this uniquely identifies the run that produces
                # the new :edge${TAG_SUFFIX} image we want to pull. `[0]` is
                # the most recent (the GitHub API sorts workflow_runs by
                # created_at descending by default), which is correct under
                # re-runs.
                RUNS=$(gh api "repos/${REPO}/actions/workflows/release.yml/runs?head_sha=${HEAD_SHA}" \
                    --jq '.workflow_runs[0] // {"status":"unknown"}' 2>/dev/null \
                    || echo '{"status":"unknown"}')

                STATUS=$(echo "$RUNS" | jq -r '.status // "unknown"')
                CONCLUSION=$(echo "$RUNS" | jq -r '.conclusion // "unknown"')

                if [ "$STATUS" = "completed" ]; then
                    if [ "$CONCLUSION" = "success" ]; then
                        echo "release.yml completed successfully after ${ELAPSED}s - :edge${TAG_SUFFIX} now resolves to ${HEAD_SHA}"
                    else
                        echo "release.yml completed with conclusion: ${CONCLUSION}"
                        echo "Falling back to :edge${TAG_SUFFIX} as-is (may be the previous commit's binary)"
                    fi
                    break
                fi

                if [ "$STATUS" = "queued" ] || [ "$STATUS" = "in_progress" ]; then
                    echo "release.yml ${STATUS}... waiting (${ELAPSED}s/${MAX_WAIT}s)"
                elif [ "$STATUS" = "unknown" ]; then
                    echo "release.yml run for ${HEAD_SHA} not found yet... waiting (${ELAPSED}s/${MAX_WAIT}s)"
                else
                    echo "release.yml status: ${STATUS}... waiting (${ELAPSED}s/${MAX_WAIT}s)"
                fi

                sleep $WAIT_INTERVAL
                ELAPSED=$((ELAPSED + WAIT_INTERVAL))
            done

            if [ $ELAPSED -ge $MAX_WAIT ]; then
                echo "Timeout waiting for release.yml after ${MAX_WAIT}s"
                echo "Falling back to :edge${TAG_SUFFIX} as-is (may be the previous commit's binary)"
            fi

            # Same final tag in all three exit paths (success, failure,
            # timeout). Only the timing and the log message differ — the
            # caller's `docker pull ghcr.io/.../merod:edge${TAG_SUFFIX}`
            # works either way because :edge${TAG_SUFFIX} always exists
            # (it points at the previous master commit's binary if the
            # current one's release.yml hasn't repointed it yet).
            TAG="edge${TAG_SUFFIX}"
        fi
    fi
else
    TAG="edge${TAG_SUFFIX}"
fi

echo "tag=${TAG}" >> "$GITHUB_OUTPUT"
if [ "$PROFILING_MODE" = "--profiling" ]; then
    echo "profiling=true" >> "$GITHUB_OUTPUT"
else
    echo "profiling=false" >> "$GITHUB_OUTPUT"
fi
echo "Using Docker image: ghcr.io/calimero-network/merod:${TAG} (${IMAGE_TYPE})"

