#!/usr/bin/env bash
# Fails while any retired bytecode/hash identifier survives in tracked sources.
# One name per concept: ApplicationId, BytecodeId, ContentHash.
set -euo pipefail

# The scan is over tracked files: a non-git checkout must fail loudly here
# rather than silently pass a gate that never greps anything.
# `git grep` only scans below the cwd, so a run from anywhere else would pass
# vacuously.
cd "$(git rev-parse --show-toplevel)"

# Retired spellings are legitimate only as back-compat shims: serde/clap
# aliases, wire-name renames, and the CLI reference's accepted flag alias.
readonly DELIBERATE='(alias|rename) = "(appKey|app-key|app_key)"|alias: `--app-key`|// wire-pin'

# Stems, not whole identifiers, so a compound name cannot slip past.
banned=(
  'app_key'
  'AppKey'
  'appkey'
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
