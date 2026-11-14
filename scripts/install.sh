#!/bin/sh
# Install Calimero's command line tools from a core release:
#
#   curl -fsSL https://raw.githubusercontent.com/calimero-network/core/master/scripts/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- merod meroctl --version 1.0.0-rc.24
#
# Installs `meroctl` (default) and/or `merod` into $CALIMERO_INSTALL_DIR
# (default ~/.local/bin), each checked against the release's SHA256SUMS before
# it is put in place. Without --version it takes the newest release, release
# candidates included. `merod` ships for Linux, and for macOS (Apple silicon)
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

if [ -z "$VERSION" ]; then
  # The newest release, pre-releases included (every 1.0 release so far is an rc).
  VERSION=$(curl -fsSL "$RELEASES_API?per_page=1" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
  [ -n "$VERSION" ] || die "could not find the newest release at $RELEASES_API (give --version)"
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM
BASE="$RELEASES/download/$VERSION"
curl -fsSL "$BASE/SHA256SUMS" -o "$TMP/SHA256SUMS" \
  || die "release $VERSION has no SHA256SUMS at $BASE (is the version right?)"

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
