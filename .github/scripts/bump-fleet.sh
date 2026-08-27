#!/usr/bin/env bash
#
# Rewrite a consumer repository's pinned Calimero dependency versions, in place.
# Edits files only - no git, no pull requests; runnable by hand to preview a bump.
#
#   bump-fleet.sh --surface cargo --version 0.11.0-rc.26 --dir ../mero-design --no-lock --dry-run
#   bump-fleet.sh --surface npm --pkg @calimero-network/mero-ui=1.5.1 --dir ../mero-meet --no-lock --dry-run
#
# Exit status is the contract with the caller:
#
#   0  files changed - open a pull request
#   3  not applicable - this repository has nothing this surface touches
#   4  already at the requested version - nothing to do
#   1  something is wrong - do NOT open a pull request
#
# Portability: macOS bash 3.2 and ubuntu runners - no mapfile, no associative
# arrays, no GNU-only sed; rewrites go through perl.

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

# Not `git add -A`: package managers write files of their own as side effects
# (e.g. pnpm's minimum-release-age workspace file), and those must not ride a bump.
CHANGED=""
record_change() {
  case " $CHANGED " in
    *" $1 "*) ;;
    *) CHANGED="$CHANGED $1" ;;
  esac
}

usage() {
  sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
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

# --dry-run reverts tracked files via `git checkout`, which would also revert
# unrelated work - so refuse on a dirty tree. Untracked files are ignored.
if [ "$DRY_RUN" -eq 1 ]; then
  if ! ( cd "$DIR" && git rev-parse --git-dir >/dev/null 2>&1 ); then
    die "--dry-run needs '$DIR' to be a git checkout (it reverts by checking out)"
  fi
  if ! ( cd "$DIR" && git diff --quiet ); then
    die "--dry-run refuses to run: '$DIR' has uncommitted changes to tracked files.
Commit or stash them first — the revert at the end cannot tell them from ours."
  fi
fi

# Three matches live in Cargo.toml: [dependencies], a second calimero-storage
# line under [dev-dependencies], and [package.metadata.calimero] min-runtime-version.
# The git URL is spelled with and without .git across repos; match both.

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

  # Verify by re-reading: comments legitimately mention old versions, so the check
  # is that no core dependency still carries a different tag.
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

  # min-runtime-version must move with the tag: if the key exists it must agree;
  # if it does not exist, say so rather than shipping a bundle with no floor.
  if grep -q 'min-runtime-version' "$manifest"; then
    local floor
    floor=$(perl -ne 'if (m{^\s*min-runtime-version\s*=\s*"([^"]*)"}) { print "$1\n"; exit }' "$manifest")
    [ "$floor" = "$VERSION" ] || die "min-runtime-version is '$floor', expected '$VERSION'"
    note "min-runtime-version now $VERSION"
  else
    note "WARNING: no min-runtime-version key — this bundle declares no runtime floor"
  fi

  record_change "logic/Cargo.toml"

  if [ "$NO_LOCK" -eq 1 ]; then
    note "skipping Cargo.lock refresh (--no-lock)"
    return 0
  fi

  command -v cargo >/dev/null 2>&1 || die "cargo is not installed; re-run with --no-lock to edit the manifest only"

  # deploy-bundle.yml records the resolved core revision from Cargo.lock on the
  # published release, so the lock is part of the bump.
  note "refreshing logic/Cargo.lock"
  ( cd "$DIR/logic" && cargo generate-lockfile --quiet )
  record_change "logic/Cargo.lock"
}

# The desktop app ships a merod BINARY (merod-config.json names the version,
# src-tauri/build.rs embeds it), so a core release means: bundle the new merod
# and cut a new desktop version that ships it.

# Highest of two dotted versions, compared numerically field by field. Not
# `sort -V`: that is a GNU extension this has to run without on macOS.
max_version() {
  printf '%s\n%s\n' "$1" "$2" \
    | awk -F. '{ printf "%010d%010d%010d %s\n", $1, $2, $3, $0 }' \
    | sort | tail -1 | cut -d' ' -f2
}

read_json_version() {
  perl -ne 'if (m{"version"\s*:\s*"([^"]*)"}) { print "$1\n"; exit }' "$1"
}

# Only the FIRST "version" key, which in all three of these files is the
# document's own version. The guard has to be set by a successful substitution,
# not by merely visiting a line — incrementing per line spends the flag on the
# opening brace and then never rewrites anything.
write_json_version() {
  NEW="$2" perl -i -pe '
    if (!$done && s{("version"\s*:\s*")[^"]*(")}{$1$ENV{NEW}$2}) { $done = 1 }
  ' "$1"
}

bump_tauri() {
  local cfg="$DIR/merod-config.json"

  if [ ! -f "$cfg" ]; then
    note "no merod-config.json — this repository bundles no merod"
    exit 3
  fi

  local current
  current=$(read_json_version "$cfg")
  [ -n "$current" ] || die "merod-config.json carries no version field"

  head_note "tauri: bundled merod $current -> $VERSION"

  if [ "$current" = "$VERSION" ]; then
    note "already bundling $VERSION"
    exit 4
  fi

  write_json_version "$cfg" "$VERSION"
  [ "$(read_json_version "$cfg")" = "$VERSION" ] || die "merod-config.json did not take the new version"
  record_change "merod-config.json"
  note "merod-config.json now bundles $VERSION"

  local pkg="apps/desktop/package.json"
  local conf="apps/desktop/src-tauri/tauri.conf.json"
  local vp="" vc="" top next

  [ -f "$DIR/$pkg" ]  && vp=$(read_json_version "$DIR/$pkg")
  [ -f "$DIR/$conf" ] && vc=$(read_json_version "$DIR/$conf")

  if [ -z "$vp" ] && [ -z "$vc" ]; then
    note "WARNING: found no desktop app version to bump"
    return 0
  fi

  top=$(max_version "${vp:-0.0.0}" "${vc:-0.0.0}")
  next=$(printf '%s' "$top" | awk -F. '{printf "%d.%d.%d", $1, $2, $3 + 1}')

  if [ -n "$vp" ] && [ -n "$vc" ] && [ "$vp" != "$vc" ]; then
    note "NOTE: app version had drifted ($pkg $vp vs $conf $vc); taking $top and syncing both"
  fi

  if [ -n "$vp" ]; then
    write_json_version "$DIR/$pkg" "$next"; record_change "$pkg"
  fi
  if [ -n "$vc" ]; then
    write_json_version "$DIR/$conf" "$next"; record_change "$conf"
  fi
  note "desktop app $top -> $next"
}

# No layout table - it would drift. Find every package.json that declares the
# dependency and walk up to the nearest lockfile for its install root.

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

      # Keep the repo's range operator: rewriting "^1.4.0" as "1.5.1" converts a
      # tracking range into a hard pin.
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

      # A major jump is a migration, not a bump - report and move on.
      # --allow-major is for a deliberate migration.
      if [ "$current_major" != "$new_major" ] && [ "$ALLOW_MAJOR" -eq 0 ]; then
        note "$rel: $name $current -> $ver crosses a major — skipped (use --allow-major)"
        skipped_major="$skipped_major $name:$current->$ver"
        continue
      fi

      NAME="$name" VAL="$prefix$ver" perl -i -pe '
        s{("\Q$ENV{NAME}\E"\s*:\s*")[^"]*(")}{$1$ENV{VAL}$2}g;
      ' "$m"
      note "$rel: $name $current -> $prefix$ver"
      record_change "$rel"
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
    # Recorded paths must be repo-relative for `git add --pathspec-from-file`;
    # a lockfile root that IS $DIR strips to itself, not to an absolute path.
    local rel="${root#$DIR}"; rel="${rel#/}"
    local lock="${rel:+$rel/}pnpm-lock.yaml"

    note "refreshing $lock"
    # Captured but replayed in full on failure: pnpm's diagnostics (e.g.
    # ERR_PNPM_NO_MATCHING_VERSION) go to stdout and must stay visible.
    if ! ( cd "$root" && pnpm install --lockfile-only --ignore-scripts ) \
        > "$TMPDIR_RUN/pnpm.log" 2>&1; then
      note "pnpm failed refreshing $lock:"
      sed 's/^/      /' "$TMPDIR_RUN/pnpm.log" >&2
      die "lockfile refresh failed for $lock"
    fi
    record_change "$lock"
  done
}

TMPDIR_RUN=$(mktemp -d)

# Revert on every exit path, not just the happy one: a dry run dying partway
# would leave the tree dirty and the clean-tree guard stuck.
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
  tauri)
    [ -n "$VERSION" ] || die "--surface tauri needs --version"
    bump_tauri
    ;;
  *) die "--surface must be cargo, npm or tauri (got '$SURFACE')" ;;
esac

# Hand the caller exactly the paths to stage; anything else the tooling left
# behind is reported, not committed.
if [ -n "${CHANGED_FILES_OUT:-}" ]; then
  : > "$CHANGED_FILES_OUT"
  for f in $CHANGED; do printf '%s\n' "$f" >> "$CHANGED_FILES_OUT"; done
fi

if [ "$DRY_RUN" -eq 1 ]; then
  head_note ""
  head_note "--dry-run: showing the diff, then reverting it"
  ( cd "$DIR" && git --no-pager diff --stat && echo && git --no-pager diff )
fi

exit 0
