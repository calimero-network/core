#!/usr/bin/env bash
# Fails while any retired bytecode/hash identifier survives in tracked sources,
# or a module is a `mod.rs` the module rule does not allow.
# One name per concept: ApplicationId, BytecodeId, ContentHash.
set -euo pipefail

# The scan is over tracked files: a non-git checkout must fail loudly here
# rather than silently pass a gate that never greps anything.
# `git grep` only scans below the cwd, so a run from anywhere else would pass
# vacuously.
cd "$(git rev-parse --show-toplevel)"

# Retired spellings are legitimate only as back-compat shims: serde/clap
# aliases, wire-name renames, and the CLI reference's accepted flag alias.
readonly DELIBERATE='(alias|rename) = "(appKey|app-key|app_key)"|alias: `--app-key`|// wire-pin|# wire-pin'

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

# sdks/ is left out: the SDKs keep their own public API names and the history
# (CHANGELOG.md) they had as separate repositories, which spells v1's `app_key`.
readonly NOT_SDKS=':(exclude)sdks/'

for name in "${banned[@]}"; do
  if hits=$(git grep -nIE -e "$name" -- '*.rs' "$NOT_SDKS" | grep -vE "$DELIBERATE"); then
    report "$name" "$hits"
  fi
done

# Prose spells the same concept its own ways: appKey, app-key, app key.
if hits=$(git grep -nIEi -e 'app[-_ ]?key' -- '*.md' '*.mdx' '*.yml' "$NOT_SDKS" \
          | grep -vE "$DELIBERATE"); then
  report "app key in docs" "$hits"
fi

# CODING_STANDARDS.md, "Modules": a module is `foo.rs` with its children in
# `foo/`. The one exception is a helper module that several integration-test
# binaries share, `tests/common/mod.rs`: a `tests/common.rs` would be a test
# binary of its own.
if hits=$(git ls-files -- ':(glob)**/mod.rs' | grep -vE '(^|/)tests/common/mod\.rs$'); then
  echo "a module is foo.rs, not foo/mod.rs (CODING_STANDARDS.md, \"Modules\"):"
  echo "$hits"
  status=1
fi

exit $status

# timing measurement ctrl-1
