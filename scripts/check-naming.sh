#!/usr/bin/env bash
# Fails while any retired bytecode/hash identifier survives in Rust sources.
# One name per concept: ApplicationId, BytecodeId, ContentHash.
set -euo pipefail

banned=(
  'app_key'
  'AppKey'
  'InvalidAppKey'
  'activated_blob'
  'bound_blob_for_context'
  'BoundBlobSource'
  'stage_blob_for'
  'stage_target_blob'
  'pending_upgrade_stage_blob'
)

status=0
for name in "${banned[@]}"; do
  # `alias = "appKey"` and `alias = "app-key"` are the deliberate
  # back-compat shims; everything else is a leftover.
  if hits=$(grep -rn --include='*.rs' -- "$name" crates/ \
            | grep -v 'alias = "appKey"' \
            | grep -v 'alias = "app-key"'); then
    echo "retired identifier '$name' still present:"
    echo "$hits"
    status=1
  fi
done
exit $status
