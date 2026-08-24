#!/usr/bin/env bash
#
# Rewrite a consumer repository's pinned Calimero dependency versions, in place.
#
# This script edits files and does NOTHING else: no git, no pull request, no
# knowledge of GitHub. That separation is the point. The workflow that calls it
# is only reachable by cutting a release, but this is runnable by hand against
# any local checkout — which is the only honest way to see what a bump would do
# before it lands as ten pull requests.
#
#   bump-fleet.sh --surface cargo --version 0.11.0-rc.26 --dir ../mero-design --no-lock --dry-run
#   bump-fleet.sh --surface npm --pkg @calimero-network/mero-ui=1.5.1 --dir ../mero-meet --no-lock --dry-run
#
# Exit status is the contract with the caller:
#
#   0  files changed — open a pull request
#   3  not applicable — this repository has nothing this surface touches
#   4  already at the requested version — nothing to do
#   1  something is wrong — do NOT open a pull request
#
# 3 and 4 are deliberately distinct from 0 and from each other. A fleet where
# "nothing happened" and "this repo does not participate" look identical is a
# fleet where a repo can silently fall out of the rollout and nobody notices.
#
# Portability: this runs on the macOS bash 3.2 a developer has and on the
# ubuntu-latest bash a runner has. No mapfile, no associative arrays, no GNU-only
# sed. Rewrites go through perl, which behaves the same in both places.

set -euo pipefail

SURFACE=""
VERSION=""
DIR=""
DRY_RUN=0
NO_LOCK=0
ALLOW_MAJOR=0
PKGS=""

die() { printf '::error::%s\n' "$*" >&2; exit 1; }
note() { printf '  %s\n' "$*" >&2; }
head_note() { printf '%s\n' "$*" >&2; }

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
  exit 1
}

while [ $# -gt 0 ]; do
  case "$1" in
    --surface) SURFACE="${2:-}"; shift 2 ;;
    --version) VERSION="${2:-}"; shift 2 ;;
    --dir) DIR="${2:-}"; shift 2 ;;
    --pkg) PKGS="$PKGS ${2:-}"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --no-lock) NO_LOCK=1; shift ;;
    --allow-major) ALLOW_MAJOR=1; shift ;;
    -h|--help) usage ;;
    *) die "unknown argument: $1" ;;
  esac
done

[ -n "$SURFACE" ] || die "--surface is required (cargo|npm)"
[ -n "$DIR" ] || die "--dir is required"
[ -d "$DIR" ] || die "--dir '$DIR' is not a directory"

# Canonical, so the lockfile-root walk below can compare paths and know when it
# has reached the top of the repository.
DIR=$(cd "$DIR" && pwd -P)

# --dry-run finishes by throwing the edits away with `git checkout`, which would
# take any unrelated work in the tree with them. Refuse rather than gamble with
# someone's uncommitted afternoon. Untracked files are ignored — nothing here
# removes those.
if [ "$DRY_RUN" -eq 1 ]; then
  if ! ( cd "$DIR" && git rev-parse --git-dir >/dev/null 2>&1 ); then
    die "--dry-run needs '$DIR' to be a git checkout (it reverts by checking out)"
  fi
  if ! ( cd "$DIR" && git diff --quiet ); then
    die "--dry-run refuses to run: '$DIR' has uncommitted changes to tracked files.
Commit or stash them first — the revert at the end cannot tell them from ours."
  fi
fi

# ---------------------------------------------------------------------------
# cargo
# ---------------------------------------------------------------------------
#
# Three separate edits live in logic/Cargo.toml, and a naive rewrite of the
# first match gets one of the three:
#
#   [dependencies]      calimero-sdk / calimero-storage / calimero-storage-macros
#                       / calimero-wasm-abi, each { git = ".../core", tag = "..." }
#   [dev-dependencies]  a SECOND calimero-storage line, features = ["testing"]
#   [package.metadata.calimero]
#                       min-runtime-version — the floor a node checks before it
#                       will accept the bundle at all. Not decoration.
#
# The git URL is spelled ".../core" in most repos and ".../core.git" in merraria
# and mero-blocks, so the matcher tolerates both.

bump_cargo() {
  local manifest="$DIR/logic/Cargo.toml"

  if [ ! -f "$manifest" ]; then
    note "no logic/Cargo.toml — this repository has no contract to bump"
    exit 3
  fi

  local current
  current=$(perl -ne '
    if (m{git\s*=\s*"https://github\.com/calimero-network/core(?:\.git)?"}
        && m{tag\s*=\s*"([^"]*)"}) { print "$1\n"; exit }
  ' "$manifest")

  if [ -z "$current" ]; then
    note "logic/Cargo.toml pins no calimero-network/core git dependency"
    exit 3
  fi

  head_note "cargo: $current -> $VERSION"

  if [ "$current" = "$VERSION" ]; then
    note "already pinned to $VERSION"
    exit 4
  fi

  NEW="$VERSION" perl -i -pe '
    if (m{git\s*=\s*"https://github\.com/calimero-network/core(?:\.git)?"}
        && m{tag\s*=\s*"}) {
      s{(tag\s*=\s*")[^"]*(")}{$1$ENV{NEW}$2};
    }
    s{^(\s*min-runtime-version\s*=\s*")[^"]*(")}{$1$ENV{NEW}$2};
  ' "$manifest"

  # Verify by re-reading, not by trusting the rewrite. Checking for a literal
  # occurrence of the old string would be wrong: comments legitimately mention
  # older versions, and a stale sentence is not a reason to abort a release.
  # What matters is that no core dependency still carries a different tag.
  local stale
  stale=$(NEW="$VERSION" perl -ne '
    if (m{git\s*=\s*"https://github\.com/calimero-network/core(?:\.git)?"}
        && m{tag\s*=\s*"([^"]*)"} && $1 ne $ENV{NEW}) { print "    line $.: $_" }
  ' "$manifest")
  [ -z "$stale" ] || die "logic/Cargo.toml still pins another core tag after the rewrite:
$stale"

  local rewritten
  rewritten=$(NEW="$VERSION" perl -ne '
    if (m{git\s*=\s*"https://github\.com/calimero-network/core(?:\.git)?"}
        && m{tag\s*=\s*"\Q$ENV{NEW}\E"}) { $n++ }
    END { print $n + 0 }
  ' "$manifest")
  [ "$rewritten" -gt 0 ] || die "rewrote no dependency lines — the manifest is not shaped as expected"
  note "$rewritten dependency line(s) now pin $VERSION"

  # min-runtime-version has to move WITH the tag. It trailed by one release
  # across six repos after the rc.24 bump because it was edited by hand and the
  # hand forgot. If the key exists it must now agree; if it does not exist,
  # say so rather than silently shipping a bundle with no floor.
  if grep -q 'min-runtime-version' "$manifest"; then
    local floor
    floor=$(perl -ne 'if (m{^\s*min-runtime-version\s*=\s*"([^"]*)"}) { print "$1\n"; exit }' "$manifest")
    [ "$floor" = "$VERSION" ] || die "min-runtime-version is '$floor', expected '$VERSION'"
    note "min-runtime-version now $VERSION"
  else
    note "WARNING: no min-runtime-version key — this bundle declares no runtime floor"
  fi

  if [ "$NO_LOCK" -eq 1 ]; then
    note "skipping Cargo.lock refresh (--no-lock)"
    return 0
  fi

  command -v cargo >/dev/null 2>&1 || die "cargo is not installed; re-run with --no-lock to edit the manifest only"

  # deploy-bundle.yml reads the resolved core revision out of Cargo.lock and
  # records it on the published release. A stale lockfile makes that provenance
  # line name the wrong commit, so the lock is part of the bump, not an
  # afterthought.
  note "refreshing logic/Cargo.lock"
  ( cd "$DIR/logic" && cargo generate-lockfile --quiet )
}

# ---------------------------------------------------------------------------
# npm
# ---------------------------------------------------------------------------
#
# Layouts differ and there is no table of them here on purpose. A table of "this
# repo keeps its frontend at app/, that one at apps/desktop/" is a thing that
# drifts silently. Instead: find every package.json that actually declares the
# dependency, and for each, walk up to the nearest lockfile to find the install
# root. That covers the standalone app/ repos, the pnpm workspaces (tauri-app,
# app-registry, mero-issue-tracker), and anything added later, without being
# told about any of them.

find_lock_root() {
  local d="$1"
  while :; do
    if [ -f "$d/pnpm-lock.yaml" ]; then printf '%s\n' "$d"; return 0; fi
    if [ "$d" = "$DIR" ] || [ "$d" = "/" ]; then return 1; fi
    d=$(dirname "$d")
  done
}

bump_npm() {
  [ -n "$PKGS" ] || die "--surface npm needs at least one --pkg name=version"

  local manifests
  manifests=$(find "$DIR" -name package.json \
    -not -path '*/node_modules/*' -not -path '*/.git/*' | sort)
  [ -n "$manifests" ] || { note "no package.json anywhere — nothing to bump"; exit 3; }

  local changed_manifests=""
  local touched=0
  local applicable=0
  local skipped_major=""

  for spec in $PKGS; do
    local name="${spec%%=*}"
    local ver="${spec#*=}"
    [ -n "$name" ] && [ -n "$ver" ] && [ "$name" != "$ver" ] \
      || die "--pkg expects name=version, got '$spec'"

    local new_major="${ver%%.*}"

    printf '%s\n' "$manifests" | while IFS= read -r m; do
      [ -n "$m" ] || continue
      local current
      current=$(NAME="$name" perl -ne '
        if (m{"\Q$ENV{NAME}\E"\s*:\s*"([^"]*)"}) { print "$1\n"; exit }
      ' "$m")
      [ -n "$current" ] || continue
      printf '%s\t%s\t%s\n' "$m" "$current" "$ver"
    done > "$TMPDIR_RUN/hits.$$"

    while IFS="$(printf '\t')" read -r m current _; do
      [ -n "${m:-}" ] || continue
      applicable=1

      local rel="${m#$DIR/}"

      if [ "$current" = "$ver" ]; then
        note "$rel: $name already $ver"
        continue
      fi

      # Keep whatever range operator the repo chose. Rewriting "^1.4.0" as
      # "1.5.1" quietly converts a tracking range into a hard pin, and the
      # repo never sees another patch release again.
      local prefix
      prefix=$(printf '%s' "$current" | sed 's/[0-9].*$//')
      local current_digits
      current_digits=$(printf '%s' "$current" | sed 's/^[^0-9]*//')
      local current_major="${current_digits%%.*}"

      case "$current_major" in
        ''|*[!0-9]*)
          note "$rel: $name is '$current', which this script will not try to interpret — skipped"
          continue ;;
      esac

      # A major jump is a migration, not a bump. mero-js is at 13 while most of
      # the fleet is on 7; opening that as an automatic pull request produces a
      # PR that cannot pass CI, every release, forever. Report it and move on.
      # --allow-major is for when someone is deliberately doing the migration.
      if [ "$current_major" != "$new_major" ] && [ "$ALLOW_MAJOR" -eq 0 ]; then
        note "$rel: $name $current -> $ver crosses a major — skipped (use --allow-major)"
        skipped_major="$skipped_major $name:$current->$ver"
        continue
      fi

      NAME="$name" VAL="$prefix$ver" perl -i -pe '
        s{("\Q$ENV{NAME}\E"\s*:\s*")[^"]*(")}{$1$ENV{VAL}$2}g;
      ' "$m"
      note "$rel: $name $current -> $prefix$ver"
      touched=$((touched + 1))
      case " $changed_manifests " in
        *" $m "*) ;;
        *) changed_manifests="$changed_manifests $m" ;;
      esac
    done < "$TMPDIR_RUN/hits.$$"
    rm -f "$TMPDIR_RUN/hits.$$"
  done

  if [ "$applicable" -eq 0 ]; then
    note "no package.json declares any of the requested packages"
    exit 3
  fi

  if [ "$touched" -eq 0 ]; then
    if [ -n "$skipped_major" ]; then
      note "every candidate was a major jump:$skipped_major"
      exit 4
    fi
    note "everything is already at the requested version"
    exit 4
  fi

  if [ "$NO_LOCK" -eq 1 ]; then
    note "skipping lockfile refresh (--no-lock)"
    return 0
  fi

  command -v pnpm >/dev/null 2>&1 || die "pnpm is not installed; re-run with --no-lock to edit manifests only"

  local roots=""
  for m in $changed_manifests; do
    local root
    if root=$(find_lock_root "$(dirname "$m")"); then
      case " $roots " in
        *" $root "*) ;;
        *) roots="$roots $root" ;;
      esac
    else
      note "WARNING: no pnpm-lock.yaml above ${m#$DIR/} — its lockfile will not be refreshed"
    fi
  done

  for root in $roots; do
    note "refreshing ${root#$DIR/}/pnpm-lock.yaml"
    # --lockfile-only resolves and writes the lock without materialising
    # node_modules. --ignore-scripts because nothing here should run a
    # dependency's install hook on a release runner.
    #
    # Captured rather than streamed, but replayed in full on failure. pnpm
    # writes its diagnostics to stdout, so discarding it outright hides the
    # single most likely failure in this script behind a bare non-zero exit —
    # including the useful case where the requested version simply is not
    # published yet, which reads as "the tooling is broken" if you cannot see
    # ERR_PNPM_NO_MATCHING_VERSION.
    if ! ( cd "$root" && pnpm install --lockfile-only --ignore-scripts ) \
        > "$TMPDIR_RUN/pnpm.log" 2>&1; then
      note "pnpm failed refreshing ${root#$DIR/}/pnpm-lock.yaml:"
      sed 's/^/      /' "$TMPDIR_RUN/pnpm.log" >&2
      die "lockfile refresh failed for ${root#$DIR/}"
    fi
  done
}

TMPDIR_RUN=$(mktemp -d)

# The --dry-run revert lives here rather than at the end of the happy path. A
# dry run that dies partway — an unpublished version, a manifest this script
# will not interpret — would otherwise leave the checkout modified, and the
# clean-tree guard above then refuses to run again until someone tidies up by
# hand. The guard proved the tree was clean before any edit, so reverting
# everything tracked is safe on any exit path.
cleanup() {
  status=$?
  rm -rf "$TMPDIR_RUN"
  if [ "$DRY_RUN" -eq 1 ]; then
    ( cd "$DIR" && git checkout -- . ) >/dev/null 2>&1 || true
  fi
  exit "$status"
}
trap cleanup EXIT

case "$SURFACE" in
  cargo)
    [ -n "$VERSION" ] || die "--surface cargo needs --version"
    bump_cargo
    ;;
  npm)
    bump_npm
    ;;
  *) die "--surface must be cargo or npm (got '$SURFACE')" ;;
esac

if [ "$DRY_RUN" -eq 1 ]; then
  head_note ""
  head_note "--dry-run: showing the diff, then reverting it"
  ( cd "$DIR" && git --no-pager diff --stat && echo && git --no-pager diff )
fi

exit 0
