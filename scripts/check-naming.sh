#!/usr/bin/env bash
# Fails while any retired bytecode/hash identifier survives in tracked sources.
# One name per concept: ApplicationId, BytecodeId, ContentHash.
set -euo pipefail

# The scan is over tracked files: a non-git checkout must fail loudly here
# rather than silently pass a gate that never greps anything.
git rev-parse --is-inside-work-tree >/dev/null

# The retired spellings are legitimate only as data: a serde/clap
# `alias = "app_key"`, the back-compat tests pinning it, and the CLI reference
# listing an accepted flag alias. Identifiers and prose are not exempt.
readonly DELIBERATE='"(appKey|app-key|app_key)"|alias: `--app-key`'

# Stems, not whole identifiers, so a compound name cannot slip past.
banned=(
  'app_key'
  'AppKey'
  'activated_blob'
  'bound_blob'
  'BoundBlobSource'
  'stage_blob'
  'stage_target_blob'
)

status=0
report() {
  echo "retired bytecode naming still present ($1):"
  echo "$2"
  status=1
}

for name in "${banned[@]}"; do
  if hits=$(git grep -nIE -e "$name" -- '*.rs' | grep -vE "$DELIBERATE"); then
    report "$name" "$hits"
  fi
done

# Prose spells the same concept its own ways: appKey, app-key, app key.
if hits=$(git grep -nIEi -e 'app[-_ ]?key' -- '*.md' '*.mdx' '*.yml' \
          | grep -vE "$DELIBERATE"); then
  report "app key in docs" "$hits"
fi

exit $status
