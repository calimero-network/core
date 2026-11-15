#!/bin/sh
# Install Calimero's command line tools from a core release:
#
#   curl -fsSL https://raw.githubusercontent.com/calimero-network/core/master/scripts/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- merod meroctl --version 1.0.0-rc.24
#
# Installs `meroctl` (default) and/or `merod` into $CALIMERO_INSTALL_DIR
# (default ~/.local/bin), each checked against the release's SHA256SUMS before
# it is put in place. Without --version it takes the newest release, release
# candidates included, whose archives are all uploaded. `merod` ships for Linux, and for macOS (Apple silicon)
# from 1.0.0-rc.25; with an older release it is skipped on a Mac, and
# `meroctl local up` runs the node in the release's Docker image either way.
#
# Options:
#   --version X   the release (a tag such as 1.0.0-rc.24)
#   --dir DIR     where to install (also CALIMERO_INSTALL_DIR)
#   -h, --help    this text
#
# CALIMERO_RELEASES (default https://github.com/calimero-network/core/releases)
# and CALIMERO_RELEASES_API (default https://api.github.com/repos/calimero-network/core/releases)
# point it elsewhere; the tests use them.
set -eu

RELEASES="${CALIMERO_RELEASES:-https://github.com/calimero-network/core/releases}"
RELEASES_API="${CALIMERO_RELEASES_API:-https://api.github.com/repos/calimero-network/core/releases}"
DIR="${CALIMERO_INSTALL_DIR:-$HOME/.local/bin}"
VERSION=""
TOOLS=""

say() { printf '%s\n' "$*" >&2; }
die() { say "install.sh: $*"; exit 1; }
usage() { sed -n '2,23p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
    --version=*) VERSION="${1#*=}"; shift ;;
    --dir) [ $# -ge 2 ] || die "--dir needs a value"; DIR="$2"; shift 2 ;;
    --dir=*) DIR="${1#*=}"; shift ;;
    -h|--help) usage; exit 0 ;;
    merod|meroctl) TOOLS="$TOOLS $1"; shift ;;
    *) die "unknown argument: $1 (the tools are merod and meroctl)" ;;
  esac
done
[ -n "$TOOLS" ] || TOOLS="meroctl"

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"
if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  die "sha256sum or shasum is required"
fi

# CALIMERO_INSTALL_MACHINE and CALIMERO_INSTALL_OS stand in for `uname -m` and
# `uname -s` in the tests.
MACHINE="${CALIMERO_INSTALL_MACHINE:-$(uname -m)}"
case "$MACHINE" in
  x86_64|amd64) ARCH=x86_64 ;;
  arm64|aarch64) ARCH=aarch64 ;;
  *) die "unsupported architecture: $MACHINE" ;;
esac
OS="${CALIMERO_INSTALL_OS:-$(uname -s)}"
case "$OS" in
  Linux) TARGET="$ARCH-unknown-linux-gnu" ;;
  Darwin)
    [ "$ARCH" = aarch64 ] || die "on macOS only Apple silicon (aarch64) builds are released"
    TARGET="aarch64-apple-darwin" ;;
  *) die "unsupported system: $OS (Linux and macOS)" ;;
esac

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

# Whether release $1 can be installed: its SHA256SUMS, and every archive of
# TOOLS it lists, can be fetched. GitHub lists a release as soon as the release
# workflow creates it, while its archives are still uploading, so the newest
# release is not always complete. Sets MISSING to what is not there yet.
release_ready() {
  MISSING=""
  curl -fsSL "$RELEASES/download/$1/SHA256SUMS" -o "$TMP/SHA256SUMS" 2>/dev/null \
    || { MISSING=SHA256SUMS; return 1; }
  for tool in $TOOLS; do
    archive="${tool}_${TARGET}.tar.gz"
    awk -v f="$archive" '$2 == f || $2 == "*"f { found = 1 } END { exit !found }' "$TMP/SHA256SUMS" || continue
    # One byte, not a HEAD: a release asset redirects to a URL signed for GET.
    curl -fsSL -r 0-0 -o /dev/null "$RELEASES/download/$1/$archive" 2>/dev/null \
      || { MISSING="$archive"; return 1; }
  done
}

if [ -z "$VERSION" ]; then
  # The newest complete release, pre-releases included (every 1.0 release so far
  # is an rc), among the newest five.
  TAGS=$(curl -fsSL "$RELEASES_API?per_page=5" \
    | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' \
    | sed 's/.*"\([^"]*\)"$/\1/')
  [ -n "$TAGS" ] || die "could not find the newest release at $RELEASES_API (give --version)"
  for tag in $TAGS; do
    if release_ready "$tag"; then VERSION="$tag"; break; fi
    say "$tag is still being published (no $MISSING yet): looking at the release before it"
  done
  [ -n "$VERSION" ] || die "none of the newest releases ($(printf '%s' "$TAGS" | tr '\n' ' ')) is complete; try again in a few minutes, or give --version"
elif ! release_ready "$VERSION"; then
  BASE="$RELEASES/download/$VERSION"
  [ "$MISSING" = SHA256SUMS ] && die "release $VERSION has no SHA256SUMS at $BASE (is the version right?)"
  die "release $VERSION has no $MISSING yet: it is still being published; try again in a few minutes"
fi

BASE="$RELEASES/download/$VERSION"
mkdir -p "$DIR"
for tool in $TOOLS; do
  archive="${tool}_${TARGET}.tar.gz"
  want=$(awk -v f="$archive" '$2 == f || $2 == "*"f { print $1 }' "$TMP/SHA256SUMS")
  if [ -z "$want" ] && [ "$tool" = merod ] && [ "$OS" = Darwin ]; then
    say "skipping merod: $VERSION has no macOS build (from 1.0.0-rc.25); meroctl local up runs the node in Docker either way"
    continue
  fi
  [ -n "$want" ] || die "release $VERSION has no $archive"
  say "downloading $tool $VERSION ($TARGET)"
  curl -fsSL "$BASE/$archive" -o "$TMP/$archive" || die "downloading $archive failed"
  got=$(sha256 "$TMP/$archive")
  [ "$got" = "$want" ] || die "$archive does not match SHA256SUMS (got $got, want $want)"
  tar -xzf "$TMP/$archive" -C "$TMP" "$tool" || die "$archive holds no $tool"
  chmod 0755 "$TMP/$tool"
  mv -f "$TMP/$tool" "$DIR/$tool"
  say "installed $DIR/$tool"
done

case ":$PATH:" in
  *":$DIR:"*) ;;
  *) say "add $DIR to your PATH, e.g.: export PATH=\"$DIR:\$PATH\"" ;;
esac
