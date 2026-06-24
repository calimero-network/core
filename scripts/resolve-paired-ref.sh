#!/usr/bin/env bash
# Resolve which mero-js ref this core change should be tested against, so a
# breaking wire change can be paired with its SDK fix and go green together
# ("core breaks first"). Resolution order:
#   1. `sdk-ref: <ref>` line in the PR body (env PR_BODY)
#   2. a same-named branch on mero-js (env HEAD_BRANCH)
#   3. the default (arg $1, or "master")
#
# Prints the resolved ref to stdout.
set -euo pipefail

DEFAULT_REF="${1:-master}"
SDK_REPO="${SDK_REPO:-https://github.com/calimero-network/mero-js.git}"

# 1. explicit `sdk-ref:` in the PR body
if [ -n "${PR_BODY:-}" ]; then
  ref="$(printf '%s\n' "$PR_BODY" | sed -n 's/^[[:space:]]*sdk-ref:[[:space:]]*\([^[:space:]]*\).*/\1/p' | head -n1)"
  if [ -n "$ref" ]; then
    echo "$ref"
    exit 0
  fi
fi

# 2. same-named branch on mero-js
if [ -n "${HEAD_BRANCH:-}" ] && \
   git ls-remote --exit-code --heads "$SDK_REPO" "$HEAD_BRANCH" >/dev/null 2>&1; then
  echo "$HEAD_BRANCH"
  exit 0
fi

# 3. default
echo "$DEFAULT_REF"
