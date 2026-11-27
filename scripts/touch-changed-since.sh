#!/usr/bin/env bash
# Touches the tracked files that differ from the commit recorded in the marker file
# $1 (all of them when it names none or one that cannot be fetched), then records
# HEAD there. Cargo decides a build is stale by source mtime, so a cached build is
# reused only for what this leaves untouched.
set -uo pipefail

marker=$1
base=$(cat "$marker" 2>/dev/null || true)
if [ -n "$base" ] && { git cat-file -e "$base^{commit}" 2>/dev/null \
     || git fetch -q --no-tags --depth=1 origin "$base"; } \
   && changed=$(git diff --name-only "$base" HEAD); then
  printf '%s\n' "$changed" | grep -v '^$' | while IFS= read -r f; do touch -c -h -- "$f"; done
  echo "cache built from $base; $(printf '%s\n' "$changed" | grep -c . || true) files changed since"
else
  git ls-files -z | xargs -0 touch -c -h
  echo "no cache commit to compare with${base:+ ($base unreachable)}: every file counts as changed"
fi
mkdir -p "$(dirname "$marker")"
git rev-parse HEAD > "$marker"
