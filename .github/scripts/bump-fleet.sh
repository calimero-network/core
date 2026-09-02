#!/usr/bin/env bash
#
# Rewrite a consumer repository's pinned Calimero dependency versions, in place.
# Edits files only - no git, no pull requests; runnable by hand to preview a bump.
#
#   bump-fleet.sh --surface cargo --version 0.11.0-rc.26 --dir ../apps --no-lock --dry-run
#   bump-fleet.sh --surface npm --pkg @calimero-network/mero-ui=1.5.1 --dir ../mero-meet --no-lock --dry-run
#
# Fixture tests, no runner or network needed:
#
#   bash .github/scripts/tests/bump-fleet.test.sh
#
# Exit status is the contract with the caller:
#
#   0  files changed - open a pull request
#   3  not applicable - this repository has nothing this surface touches
#   4  already at the requested version - nothing to do
#   1  something is wrong - do NOT open a pull request
#
# Two optional env vars are the rest of the contract with the caller:
#
#   CHANGED_FILES_OUT  every path this run means to have changed, one per line,
#                      repository-relative. The caller stages exactly these.
#   UNCLAIMED_OUT      every core pin this run RECOGNISED and did not move,
#                      grouped by why. Empty is the normal, boring case.
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
# The git URL is spelled ".../core" in some manifests and ".../core.git" in
# others, so the matcher tolerates both.

# The monorepo lays the same surface out differently, so `cargo` means one of
# two shapes and the script decides which by looking rather than by being told.
#
#   standalone   logic/Cargo.toml            one contract, its own pins
#   workspace    Cargo.toml [workspace]      apps/*/logic, one shared pin
#
# calimero-network/apps is the second: nine contracts whose SDK tag lives once,
# in [workspace.dependencies]. That is the whole reason the monorepo exists —
# "a core release is ONE edit", per its root Cargo.toml — but a release is still
# more than the three tag lines, because the repo enforces two derived values in
# CI (scripts/check-app-metadata.sh):
#
#   min-runtime-version   the workspace value, AND a copy in every app, because
#                         [package.metadata] is not on cargo's inheritable list
#   merod-image           the workspace value, AND the image every merobox
#                         scenario starts its node from
#
# Miss either and the bump PR is red on a check that names the drift precisely.
# So the rewrite covers all of it, and then re-reads to assert what that script
# asserts — a bump should not be able to open a PR that fails a check this
# script could have run itself.

# The tag any calimero-network/core git dependency is pinned to, first match.
core_tag_in() {
  perl -ne '
    if (m{git\s*=\s*"https://github\.com/calimero-network/core(?:\.git)?"}
        && m{tag\s*=\s*"([^"]*)"}) { print "$1\n"; exit }
  ' "$1"
}

# Every apps/*/logic/Cargo.toml. Deliberately not */logic/crates/*/Cargo.toml —
# a shared crate carries no [package.metadata.calimero] and the checker treats
# it as a crate, not an app.
app_manifests() {
  find "$DIR/apps" -type f -path '*/logic/Cargo.toml' 2>/dev/null | sort
}

# Every merobox scenario. The checker globs apps/*/logic/workflows/*.yml, one
# level; this recurses, so the probes/ subdirectories that carry the same image
# and that the checker never looks at do not quietly rot.
scenario_files() {
  find "$DIR/apps" -type f -path '*/logic/workflows/*' \
       \( -name '*.yml' -o -name '*.yaml' \) 2>/dev/null | sort
}

bump_cargo_workspace() {
  local root="$DIR/Cargo.toml"

  local current
  current=$(core_tag_in "$root")
  if [ -z "$current" ]; then
    note "Cargo.toml pins no calimero-network/core git dependency"
    exit 3
  fi

  head_note "cargo (workspace): $current -> $VERSION"

  if [ "$current" = "$VERSION" ]; then
    note "already pinned to $VERSION"
    exit 4
  fi

  # ── the workspace root: dependency tags, the floor, and the node image ────
  NEW="$VERSION" perl -i -pe '
    if (m{git\s*=\s*"https://github\.com/calimero-network/core(?:\.git)?"}
        && m{tag\s*=\s*"}) {
      s{(tag\s*=\s*")[^"]*(")}{$1$ENV{NEW}$2};
    }
    s{^(\s*min-runtime-version\s*=\s*")[^"]*(")}{$1$ENV{NEW}$2};
    s{(ghcr\.io/calimero-network/merod:)[A-Za-z0-9._-]+}{$1$ENV{NEW}};
  ' "$root"
  record_change "Cargo.toml"

  local n
  n=$(NEW="$VERSION" perl -ne '
    if (m{git\s*=\s*"https://github\.com/calimero-network/core(?:\.git)?"}
        && m{tag\s*=\s*"\Q$ENV{NEW}\E"}) { $n++ }
    END { print $n + 0 }
  ' "$root")
  [ "$n" -gt 0 ] || die "rewrote no dependency lines in Cargo.toml — not shaped as expected"
  note "Cargo.toml: $n dependency line(s) now pin $VERSION"

  # ── every app's copy of the floor ────────────────────────────────────────
  local apps=0
  local m
  for m in $(app_manifests); do
    grep -q 'min-runtime-version' "$m" || continue
    NEW="$VERSION" perl -i -pe '
      s{^(\s*min-runtime-version\s*=\s*")[^"]*(")}{$1$ENV{NEW}$2};
    ' "$m"
    record_change "${m#$DIR/}"
    apps=$((apps + 1))
  done
  note "min-runtime-version rewritten in $apps app manifest(s)"

  # ── every merobox scenario's node image ──────────────────────────────────
  local scen=0
  local f
  for f in $(scenario_files); do
    grep -q 'ghcr\.io/calimero-network/merod:' "$f" || continue
    NEW="$VERSION" perl -i -pe '
      s{(ghcr\.io/calimero-network/merod:)[A-Za-z0-9._-]+}{$1$ENV{NEW}}g;
    ' "$f"
    record_change "${f#$DIR/}"
    scen=$((scen + 1))
  done
  note "merod image rewritten in $scen scenario file(s)"

  # ── the toolchain and the dev scripts ────────────────────────────────────
  # Everything above is asserted by scripts/check-app-metadata.sh in CI, so a
  # rewrite this script misses up there costs a red check on the pull request
  # it opens. Nothing below is asserted by anything — which is precisely why
  # every one of them had already drifted by the time this was written:
  #
  #   mero-stream/scripts/ensure-cargo-mero.sh   rc.25, three behind the SDK
  #   mero-calendar/Makefile                     rc.24, four behind
  #   mero-calendar/scripts/{dev-node,setup}.sh  rc.25
  #
  # Being stale here does not fail a build; it installs the WRONG TOOL. The ABI
  # emitter is versioned with core, so a cargo-mero from an older release can
  # embed an ABI describing a schema the node does not share, and a dev who
  # followed a stale `cargo install` line in a Makefile gets that silently.
  bump_workspace_toolchain

  verify_workspace_consistency

  # Workspace path only. The report itself is repo-shape agnostic, but it has
  # only been calibrated against calimero-network/apps: turning it on for the
  # standalone repos would emit an UNEXPLAINED group nobody has triaged yet,
  # and a report that cries wolf on its first run is a report people learn to
  # scroll past. Worth extending to them deliberately, one repo at a time.
  report_unclaimed

  if [ "$NO_LOCK" -eq 1 ]; then
    note "skipping Cargo.lock refresh (--no-lock)"
    return 0
  fi

  command -v cargo >/dev/null 2>&1 || die "cargo is not installed; re-run with --no-lock to edit manifests only"

  # One lockfile for the whole workspace. `cargo update -p` per crate would be
  # nine resolutions of the same graph; the workspace root resolves once.
  note "refreshing Cargo.lock"
  ( cd "$DIR" && cargo update --quiet )
  record_change "Cargo.lock"
}

# ---------------------------------------------------------------------------
# the pins nothing checks
# ---------------------------------------------------------------------------
#
# check-app-metadata.sh covers the workspace root, every app's floor and every
# apps/*/logic/workflows/*.yml. It does not cover the toolchain, and the
# toolchain is where a stale pin does its quietest damage: cargo-mero's ABI
# emitter is versioned with core, so a tool from an older release can embed an
# ABI describing a schema the node does not share, and the build is green.
#
# Shell scripts and Makefiles under apps/ are rewritten. Deliberately not:
#
#   *.md            prose. A sentence recording what some earlier release did
#                   is not drift, and rewriting it rewrites history.
#   **/logs/**      committed merobox output. Same, more so.
#   **/test/perf/** a recorded measurement. p2p-sheets' perf scenarios start a
#                   merod pinned to rc.8 and its PERFORMANCE.md reports numbers
#                   measured against rc.8; moving the image would leave a
#                   document whose stated conditions no longer describe what
#                   runs. Reported instead, which is the honest half.
tooling_files() {
  find "$DIR/apps" -type f \
       \( -name '*.sh' -o -name 'Makefile' -o -name '*.mk' \) \
       -not -path '*/node_modules/*' \
       -not -path '*/logs/*' \
       -not -path '*/test/perf/*' \
    2>/dev/null | sort
}

# Rewrite the `default:` belonging to the `version:` INPUT — not merely the
# first `default:` in the file. install-cargo-mero declares one input today, so
# "first default" was correct by luck; an input added above it would have moved
# the rewrite onto the wrong key with nothing anywhere to notice.
#
# Returns 0 if it rewrote a default, 1 if the input has none.
rewrite_action_version_default() {
  NEW="$VERSION" perl -i -ne '
    if (!$done) {
      if (m/^(\s*)version:\s*$/) { $ind = length($1); $in = 1; print; next; }
      if ($in) {
        if (m/^(\s*)\S/ && length($1) <= $ind) { $in = 0; }
        elsif (s{^(\s*default:\s*")[^"]*(")}{$1$ENV{NEW}$2}) { $done = 1; print; next; }
      }
    }
    print;
    END { exit($done ? 0 : 1) }
  ' "$1"
}

# Files this run recognised as carrying a core pin but chose not to rewrite,
# with the reason. Reported, never fatal.
CHECKSUM_PINNED=""

bump_workspace_toolchain() {
  local action=".github/actions/install-cargo-mero/action.yml"
  if [ -f "$DIR/$action" ]; then
    if rewrite_action_version_default "$DIR/$action"; then
      record_change "$action"
      note "install-cargo-mero: version input default now $VERSION"
    else
      note "WARNING: $action declares no version input default — cargo-mero is unpinned there"
    fi
  fi

  local touched=0
  local f rel installer sum
  for f in $(tooling_files); do
    rel="${f#$DIR/}"

    # Does the file say what it is installing? The bare-variable rule below is
    # only safe where it does: `VERSION=1.2.3` in an unrelated script is not a
    # core pin, and rewriting it is silent corruption rather than a bump.
    # `if grep`, not `grep && installer=1`. The second form is a command list
    # whose status is grep's when grep finds nothing, and reasoning about
    # whether `set -e` exempts that is exactly the kind of thing to not have to
    # reason about in a script that opens pull requests unattended.
    installer=0
    if grep -qE 'calimero-network/core|cargo-mero' "$f"; then installer=1; fi

    # A script that pins per-asset checksums NEXT TO the release cannot be
    # bumped by moving the version alone — the download then fails its own
    # integrity check, which is a harder break than being a release stale.
    # mero-drive/scripts/setup-cargo-mero.sh is shaped that way on purpose, so
    # it is named in the report with the command that refreshes it.
    if [ "$installer" -eq 1 ] && grep -qE 'CHECKSUM|shasum -a 256|sha256sum' "$f"; then
      CHECKSUM_PINNED="$CHECKSUM_PINNED $rel"
      continue
    fi

    sum=$(cksum < "$f")
    NEW="$VERSION" INSTALLER="$installer" perl -i -pe '
      my $v = qr/[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?/;

      # A merod container image, wherever it appears — `docker pull`, an echo
      # naming the image about to start, a compose fragment.
      #
      # Matched on `merod:<ver>` and NOT on the full ghcr.io path: the
      # Makefile in scaffolding-e2e echoes the short form on the line above the
      # `docker pull` that uses the long one. Anchoring on the registry left
      # those two lines disagreeing after every bump, which is the shape of
      # drift that teaches people not to trust the output.
      #
      # (No apostrophes in this block. It is a single-quoted shell string, and
      # one closes it — bash then reports a syntax error on the perl below.)
      s{(\bmerod:)$v}{$1$ENV{NEW}}g;

      # A bundle manifest floor written by hand. scaffolding-e2e assembles its
      # multi-service bundle with a heredoc instead of cargo-mero, so this
      # minRuntimeVersion is not one check-app-metadata.sh reads.
      s{("minRuntimeVersion"\s*:\s*")$v(")}{$1$ENV{NEW}$2}g;

      if ($ENV{INSTALLER}) {
        # `cargo install --git .../core --tag <ver> cargo-mero`, whether it runs
        # or sits in a comment telling a developer what to run. A stale comment
        # here is not cosmetic: someone follows it and installs the wrong tool.
        # NOT `?tag=<ver>`. That form is not how anything installs anything —
        # cargo takes `--tag` — it only ever appears in `cargo install --list`
        # OUTPUT quoted in a comment, next to the commit sha it resolved to.
        # Rewriting it produced a line pairing a new tag with an old sha, which
        # is worse than the stale line it replaced.
        s{(--tag\s+"?)$v}{$1$ENV{NEW}}g if m{cargo-mero|calimero-network/core};
        # The single variable naming the release being fetched.
        s{$v}{$ENV{NEW}} if m{^\s*(?:RELEASE|VERSION|CORE_TAG|CORE_RELEASE|CARGO_MERO_TAG)=};
      }
    ' "$f"

    if [ "$sum" != "$(cksum < "$f")" ]; then
      record_change "$rel"
      note "$rel: core pins now $VERSION"
      touched=$((touched + 1))
    fi
  done
  note "$touched tooling file(s) rewritten"
}

# Every occurrence of a core version pin left in the tree naming some other
# release, classified.
#
# This is not tidiness. Every silent-drift incident in this fleet looked like a
# green run: the pins something checks moved, the ones nothing checks did not,
# and no line anywhere said so. A bump that publishes its own blind spots can
# be audited; one that only reports success cannot. So the leftovers are
# grouped into the ones there is a reason for — prose, recorded output, a
# recorded measurement — and the ones there is not, which is the interesting
# list and should normally be empty.
report_unclaimed() {
  local raw
  raw=$( cd "$DIR" && NEW="$VERSION" perl -e '
    use File::Find;
    my $new = $ENV{NEW};
    my @out;
    find({ no_chdir => 0, wanted => sub {
      return unless -f $_ && -s $_ < 2_000_000;
      my $p = $File::Find::name; $p =~ s{^\./}{};
      return if $p =~ m{(^|/)(\.git|node_modules|target|dist|\.venv)/};
      return if $p =~ m{\.(lock|png|jpg|jpeg|gif|svg|ico|wasm|mpk|woff2?|zip|gz)$};
      return if $p =~ m{(^|/)(Cargo\.lock|pnpm-lock\.yaml)$};
      open my $fh, "<", $_ or return;
      my $n = 0;
      while (my $l = <$fh>) {
        $n++;
        my @v;
        push @v, $1 while $l =~ m{\bmerod:([0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?)}g;
        push @v, $1 while $l =~ m{(?:minRuntimeVersion|min-runtime-version)"?\s*[:=]\s*"([0-9][^"]*)"}g;
        push @v, $1 while $l =~ m{calimero-network/core(?:\.git)?.*?tag\s*=\s*"([^"]+)"}g;
        for my $v (@v) { push @out, "$p\t$n\t$v" if $v ne $new }
      }
      close $fh;
    }}, ".");
    print "$_\n" for @out;
  ' ) || true

  local prose="" recorded="" baseline="" unexplained=""
  local line p
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    p=${line%%	*}
    case "$p" in
      */logs/*|*.log)          recorded="$recorded
  $line" ;;
      */test/perf/*)           baseline="$baseline
  $line" ;;
      *.md|*.rs|*.ts|*.tsx|*.mjs) prose="$prose
  $line" ;;
      *)                       unexplained="$unexplained
  $line" ;;
    esac
  done <<REPORT
$raw
REPORT

  local body=""
  [ -n "$CHECKSUM_PINNED" ] && body="$body
NOT REWRITTEN — pins asset checksums next to the release, so moving the version
alone would fail the download's own integrity check. Refresh by hand:
$(for r in $CHECKSUM_PINNED; do printf '  %s\n' "$r"; done)
  shasum -a 256 cargo-mero_<target>.tar.gz   # for each CHECKSUM_ line
"
  [ -n "$baseline" ] && body="$body
NOT REWRITTEN — a recorded measurement, whose document states the conditions it
was measured under. Moving these needs a re-measure, not a bump:$baseline
"
  [ -n "$prose" ] && body="$body
NOT REWRITTEN — prose and source comments. A sentence about an earlier release
is history, not drift:$prose
"
  # Summarised, not listed. scaffolding-e2e has two committed merobox log
  # directories carrying 46 references to a merod from 0.10.1 — listing each
  # one buries the group above it, which is the group worth reading.
  #
  # `awk NF{print;exit}` and not `grep . | head -1`: this script runs under
  # `set -o pipefail`, and `head` closing the pipe early makes the whole
  # pipeline exit 141. Inside a command substitution that yields an EMPTY
  # string, so the sample line silently vanished while every count around it
  # stayed right — which is how it got noticed rather than shipped.
  if [ -n "$recorded" ]; then
    body="$body
NOT REWRITTEN — committed tool output: $(printf '%s' "$recorded" | grep -c .) reference(s) in $(printf '%s' "$recorded" | awk -F'\t' 'NF{print $1}' | sort -u | grep -c .) file(s), e.g.
$(printf '%s' "$recorded" | awk 'NF { print; exit }')
"
  fi

  if [ -n "$unexplained" ]; then
    body="$body
⚠️  UNEXPLAINED — a core pin this script recognised, did not move, and has no
reason for. Either it is a real miss (extend bump-fleet.sh) or it belongs in
one of the groups above:$unexplained
"
    printf '::warning::bump-fleet left %s unexplained core pin(s) — see the run summary\n' \
      "$(printf '%s' "$unexplained" | grep -c .)" >&2
  fi

  if [ -n "$body" ]; then
    head_note ""
    head_note "── pins this bump did NOT move ──────────────────────────────────"
    printf '%s\n' "$body" >&2
  fi

  if [ -n "${UNCLAIMED_OUT:-}" ]; then
    printf '%s' "$body" > "$UNCLAIMED_OUT"
  fi
}

# Re-read what was just written and assert the two invariants
# scripts/check-app-metadata.sh asserts in CI. Checking the rewrite rather than
# trusting it is the same reason bump_cargo_app re-reads: a regex that matched
# nothing fails silently, and the cost of finding out is a red PR per release.
verify_workspace_consistency() {
  local root="$DIR/Cargo.toml"
  local stale

  stale=$(NEW="$VERSION" perl -ne '
    if (m{git\s*=\s*"https://github\.com/calimero-network/core(?:\.git)?"}
        && m{tag\s*=\s*"([^"]*)"} && $1 ne $ENV{NEW}) { print "    line $.: $_" }
  ' "$root")
  [ -z "$stale" ] || die "Cargo.toml still pins another core tag:
$stale"

  local want_min
  want_min=$(perl -ne 'if (m{^\s*min-runtime-version\s*=\s*"([^"]*)"}) { print "$1\n"; exit }' "$root")
  [ "$want_min" = "$VERSION" ] \
    || die "workspace min-runtime-version is '${want_min:-<unset>}', expected '$VERSION'"

  local want_image
  want_image=$(perl -ne 'if (m{^\s*merod-image\s*=\s*"([^"]*)"}) { print "$1\n"; exit }' "$root")
  [ -n "$want_image" ] \
    || die "[workspace.metadata.mero-apps].merod-image is not set — the scenario check reads it"
  case "$want_image" in
    *":$VERSION") ;;
    *) die "workspace merod-image is '$want_image', expected it to end with ':$VERSION'" ;;
  esac

  local m got
  for m in $(app_manifests); do
    # Not a warning. check-app-metadata.sh rejects an app under apps/*/logic
    # with no floor, so continuing here would open a pull request that is red
    # for a reason this script already knows.
    grep -q 'min-runtime-version' "$m" \
      || die "${m#$DIR/} declares no min-runtime-version — the metadata check rejects that"
    got=$(perl -ne 'if (m{^\s*min-runtime-version\s*=\s*"([^"]*)"}) { print "$1\n"; exit }' "$m")
    [ "$got" = "$VERSION" ] \
      || die "${m#$DIR/}: min-runtime-version is '$got', workspace says '$VERSION'"
  done

  local f img
  for f in $(scenario_files); do
    for img in $(grep -oE 'ghcr\.io/calimero-network/merod:[A-Za-z0-9._-]+' "$f" | sort -u); do
      [ "$img" = "$want_image" ] \
        || die "${f#$DIR/}: runs $img, workspace declares $want_image"
    done
  done

  note "workspace, apps and scenarios all agree on $VERSION"
}

bump_cargo() {
  if [ -f "$DIR/logic/Cargo.toml" ]; then
    bump_cargo_app
  elif [ -f "$DIR/Cargo.toml" ] && grep -q 'calimero-network/core' "$DIR/Cargo.toml"; then
    bump_cargo_workspace
  else
    note "no logic/Cargo.toml, and no workspace root pinning calimero-network/core"
    exit 3
  fi
}

bump_cargo_app() {
  local manifest="$DIR/logic/Cargo.toml"

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

# ---------------------------------------------------------------------------
# npm
# ---------------------------------------------------------------------------
#
# Layouts differ and there is no table of them here on purpose. A table of "this
# repo keeps its frontend at app/, that one at apps/desktop/" is a thing that
# drifts silently. Instead: find every package.json that actually declares the
# dependency, and for each, walk up to the nearest lockfile to find the install
# root. That covers the standalone app/ repos, the pnpm workspaces (tauri-app,
# app-registry, apps), and anything added later, without being told about any of
# them. A workspace that resolves its versions through a pnpm CATALOG declares
# them nowhere near a package.json, so that is handled separately below.

find_lock_root() {
  local d="$1"
  while :; do
    if [ -f "$d/pnpm-lock.yaml" ]; then printf '%s\n' "$d"; return 0; fi
    if [ "$d" = "$DIR" ] || [ "$d" = "/" ]; then return 1; fi
    d=$(dirname "$d")
  done
}

# A pnpm CATALOG is the npm half of what [workspace.dependencies] is to cargo,
# and calimero-network/apps uses it: every app writes
#
#     "@calimero-network/mero-js": "catalog:"
#
# and the real range lives once, in pnpm-workspace.yaml. Rewriting package.json
# there changes nothing, and — worse — the literal string "catalog:" carries no
# digits, so the numeric guard below skips it and the run reports "everything is
# already at the requested version" and exits 4. A repository that silently
# stops receiving bumps while reporting success is the exact failure the exit
# codes exist to prevent, so the catalog is handled first and explicitly.
#
# Matching is not restricted to the default `catalog:` block. A named catalog
# under `catalogs:` has the same shape, and in pnpm-workspace.yaml a
# `name: version` line can only be a catalog entry — `packages:` and
# `onlyBuiltDependencies:` are sequences, not maps.
catalog_version_of() {
  NAME="$1" perl -ne '
    if (m{^\s*"?\Q$ENV{NAME}\E"?\s*:\s*"?([^"\s#]+)"?}) { print "$1\n"; exit }
  ' "$2"
}

# Rewrite one package in the catalog. Echoes the outcome as a single word so the
# caller can account for it: changed / same / absent / major / opaque.
catalog_bump() {
  local name="$1" ver="$2" ws="$3"

  local current
  current=$(catalog_version_of "$name" "$ws")
  [ -n "$current" ] || { printf 'absent\n'; return 0; }

  local prefix current_digits current_major
  prefix=$(printf '%s' "$current" | sed 's/[0-9].*$//')
  current_digits=$(printf '%s' "$current" | sed 's/^[^0-9]*//')
  current_major="${current_digits%%.*}"

  case "$current_major" in
    ''|*[!0-9]*) printf 'opaque\n'; return 0 ;;
  esac

  if [ "$current" = "$prefix$ver" ]; then printf 'same\n'; return 0; fi

  if [ "$current_major" != "${ver%%.*}" ] && [ "$ALLOW_MAJOR" -eq 0 ]; then
    printf 'major\n'; return 0
  fi

  NAME="$name" VAL="$prefix$ver" perl -i -pe '
    s{^(\s*"?\Q$ENV{NAME}\E"?\s*:\s*)"?[^"\s#]+"?(\s*(?:#.*)?)$}{$1$ENV{VAL}$2};
  ' "$ws"

  local now
  now=$(catalog_version_of "$name" "$ws")
  [ "$now" = "$prefix$ver" ] || die "pnpm-workspace.yaml: $name is '$now' after the rewrite, expected '$prefix$ver'"

  printf 'changed\n'
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
  local catalog_changed=0

  local workspace_yaml="$DIR/pnpm-workspace.yaml"
  [ -f "$workspace_yaml" ] || workspace_yaml=""

  for spec in $PKGS; do
    local name="${spec%%=*}"
    local ver="${spec#*=}"
    [ -n "$name" ] && [ -n "$ver" ] && [ "$name" != "$ver" ] \
      || die "--pkg expects name=version, got '$spec'"

    local new_major="${ver%%.*}"

    if [ -n "$workspace_yaml" ]; then
      local verdict
      verdict=$(catalog_bump "$name" "$ver" "$workspace_yaml")
      case "$verdict" in
        changed)
          applicable=1
          touched=$((touched + 1))
          catalog_changed=1
          record_change "pnpm-workspace.yaml"
          note "pnpm-workspace.yaml: $name -> $ver (catalog)" ;;
        same)
          applicable=1
          note "pnpm-workspace.yaml: $name already $ver (catalog)" ;;
        major)
          applicable=1
          note "pnpm-workspace.yaml: $name crosses a major to $ver — skipped (use --allow-major)"
          skipped_major="$skipped_major $name->$ver" ;;
        opaque)
          note "pnpm-workspace.yaml: $name is not a version this script will interpret — skipped" ;;
        absent) ;;
      esac
    fi

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

      case "$current" in
        catalog:*|workspace:*)
          # The version lives in pnpm-workspace.yaml and was handled above.
          continue ;;
      esac

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
  if [ "$catalog_changed" -eq 1 ]; then
    roots=" $DIR"
  fi
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
    # $DIR is absolute, so a nested root strips to "app", "apps/desktop", and so
    # on. A root that IS $DIR — the repository that keeps its lockfile at the top
    # level — strips to itself, because there is no "$DIR/" prefix to remove. The
    # recorded path then stayed absolute, and `git add --pathspec-from-file`,
    # which reads paths as repository-relative, failed the whole bump with
    #
    #   fatal: pathspec 'home/runner/work/.../pnpm-lock.yaml' did not match any files
    #
    # tauri-app, app-registry and the apps monorepo are all shaped that way.
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
