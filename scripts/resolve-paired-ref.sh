#!/usr/bin/env bash
# Resolve which ref of a downstream repo this core change should be tested
# against, so a breaking change can be paired with its fix and go green together
# ("core breaks first"). Resolution order:
#   1. `<REF_KEY>: <ref>` line in the PR body (env PR_BODY; REF_KEY defaults to sdk-ref)
#   2. a same-named branch on SDK_REPO (env HEAD_BRANCH; defaults to mero-js)
#   3. the default (arg $1, or "master")
#
# Prints the resolved ref to stdout.
set -euo pipefail

DEFAULT_REF="${1:-master}"
SDK_REPO="${SDK_REPO:-https://github.com/calimero-network/mero-js.git}"
REF_KEY="${REF_KEY:-sdk-ref}"

# 1. explicit `<REF_KEY>:` in the PR body
if [ -n "${PR_BODY:-}" ]; then
  ref="$(printf '%s\n' "$PR_BODY" | sed -n "/^[[:space:]]*$REF_KEY:[[:space:]]*\\([^[:space:]]*\\).*/{s//\\1/p;q;}")"
  if [ -n "$ref" ]; then
    # The ref is fed to `actions/checkout`; restrict it to a safe git-ref shape so
    # a crafted PR body can't smuggle anything through. Reject illegal characters,
    # `..` traversal, and leading `-` (option-injection) or `/`.
    case "$ref" in
      *[!A-Za-z0-9._/-]* | *..* | -* | /*)
        echo "resolve-paired-ref: refusing unsafe $REF_KEY: $ref" >&2
        exit 1
        ;;
    esac
    echo "$ref"
    exit 0
  fi
fi

# 2. same-named branch on the paired repo
if [ -n "${HEAD_BRANCH:-}" ] && \
   git ls-remote --exit-code --heads "$SDK_REPO" "$HEAD_BRANCH" >/dev/null 2>&1; then
  echo "$HEAD_BRANCH"
  exit 0
fi

# 3. default
echo "$DEFAULT_REF"
