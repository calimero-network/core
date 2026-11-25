#!/usr/bin/env bash
# Builds the deep upgrade cases' fixtures (crates/node/tests/fault/upgrade_cases/20_*.py
# and on) from a release's own source, with that release's own tools: the
# components and packages a cluster on that release would run. Run after
# fetch-release.sh and build-release-fixtures.sh, whose checkout (OUT/src) and
# tools (OUT/tools/bin/release-build and release-pack) it reuses.
#
#   scripts/fetch-release.sh 1.0.0-rc.15 target/upgrade-from
#   scripts/build-release-fixtures.sh 1.0.0-rc.15 target/upgrade-from
#   scripts/build-upgrade-deep-fixtures.sh target/upgrade-from
#
# Writes OUT/deep/: kv-dev.wasm (the release's kv fixture component from its
# kv-store-test-fixture.mpk, as a dev bundle of its own), files.wasm, and the signed
# packages kv-a-1.0.0.mpk, kv-a-1.1.0.mpk, kv-b-1.0.0.mpk, kv-c-1.0.0.mpk,
# kv-u-1.0.0.mpk, kv-u-1.1.0.mpk, kv-p-1.0.0.mpk, kv-r-1.0.0.mpk, kv-r-1.1.0.mpk, kv-o-1.0.0.mpk to kv-o-1.3.0.mpk (four releases
# of one package, a deployment's offer moving between them) (com.calimero.upgrade.kv-*), notes-1.0.0.mpk and notes-2.0.0.mpk
# (com.calimero.upgrade.notes, apps/fixture-notes-v1 and fixture-notes-v2), and, from a
# release that has the units examples, calendar-0.1.0.mpk, calendar-0.1.1.mpk,
# planner-0.1.0.mpk, planner-0.2.0.mpk and booking-0.1.0.mpk (com.calimero.example.*); from
# one whose meroctl packs them, kv-guest-1.0.0.mpk (`guests: allowed`), kv-secret-1.0.0.mpk
# (it declares a secret, `secrets`), kv-svc-1.0.0.mpk and kv-svc-1.1.0.mpk (two services, from 1.0.0-rc.25) and kv-unit-1.0.0.mpk (`uses` kv-secret, `mode = "create"`).
set -euo pipefail

OUT=$(cd "${1:?usage: build-upgrade-deep-fixtures.sh OUT}" && pwd -P)
SRC="$OUT/src"
BUILD="$OUT/tools/bin/release-build"
PACK="$OUT/tools/bin/release-pack"
DEEP="$OUT/deep"
for need in "$SRC/.git" "$BUILD" "$PACK" "$OUT/kv-store-test-fixture.mpk"; do
  test -e "$need" || { echo "build-upgrade-deep-fixtures: $need missing (fetch-release.sh and build-release-fixtures.sh first)" >&2; exit 1; }
done
mkdir -p "$DEEP"

# The release's kv component, as components of their own (one custom section each):
# kv-dev.wasm, every account's dev bundle, and one per package below
# (scripts/upgrade-dev-bundle.py says why).
python3 -I "$(dirname "$0")/upgrade-dev-bundle.py" "$OUT/kv-store-test-fixture.mpk" "$DEEP" dev a b c u r o p unit guest secret m1 m2

for app in files notes-v1 notes-v2; do
  # An older release keeps its test fixtures under apps/v2-fixtures/.
  dir="apps/fixture-$app"
  [ -d "$SRC/$dir" ] || dir="apps/v2-fixtures/$app"
  (cd "$SRC" && "$BUILD" "$dir" --out "$DEEP/$app.wasm")
done

pack() { # WASM PACKAGE VERSION OUT
  "$PACK" --wasm "$1" --package "$2" --app-version "$3" --out "$DEEP/$4"
}
pack "$DEEP/kv-a.wasm" com.calimero.upgrade.kv-a 1.0.0 kv-a-1.0.0.mpk
pack "$DEEP/kv-a.wasm" com.calimero.upgrade.kv-a 1.1.0 kv-a-1.1.0.mpk
pack "$DEEP/kv-b.wasm" com.calimero.upgrade.kv-b 1.0.0 kv-b-1.0.0.mpk
pack "$DEEP/kv-c.wasm" com.calimero.upgrade.kv-c 1.0.0 kv-c-1.0.0.mpk
pack "$DEEP/kv-u.wasm" com.calimero.upgrade.kv-u 1.0.0 kv-u-1.0.0.mpk
pack "$DEEP/kv-u.wasm" com.calimero.upgrade.kv-u 1.1.0 kv-u-1.1.0.mpk
pack "$DEEP/kv-p.wasm" com.calimero.upgrade.kv-p 1.0.0 kv-p-1.0.0.mpk
# kv-r: two releases of one WASM, the first uninstalled under its apps (`unclassified-updates`).
pack "$DEEP/kv-r.wasm" com.calimero.upgrade.kv-r 1.0.0 kv-r-1.0.0.mpk
pack "$DEEP/kv-r.wasm" com.calimero.upgrade.kv-r 1.1.0 kv-r-1.1.0.mpk
for v in 1.0.0 1.1.0 1.2.0 1.3.0; do
  pack "$DEEP/kv-o.wasm" com.calimero.upgrade.kv-o "$v" "kv-o-$v.mpk"
done
pack "$DEEP/notes-v1.wasm" com.calimero.upgrade.notes 1.0.0 notes-1.0.0.mpk
pack "$DEEP/notes-v2.wasm" com.calimero.upgrade.notes 2.0.0 notes-2.0.0.mpk

# A package of two services (`--service NAME=PATH`, from 1.0.0-rc.25): kv-svc-1.0.0.mpk and 1.1.0.
if "$PACK" --help 2>/dev/null | grep -q -- '--service'; then
  for v in 1.0.0 1.1.0; do
    "$PACK" --package com.calimero.upgrade.kv-svc --app-version "$v" \
      --service alpha="$DEEP/kv-m1.wasm" --service beta="$DEEP/kv-m2.wasm" --out "$DEEP/kv-svc-$v.mpk"
  done
else
  echo "note: this meroctl has no --service (before 1.0.0-rc.25): kv-svc is not packed" >&2
fi

# A release that admits guests (`guests: allowed`); a release that binds a secret to a header
# of its outbound host (`secrets`); and a kv unit: kv-unit's release `uses` kv-secret with
# `mode = "create"`. Each declared in a crate's metadata as an app's author would (no lock:
# kv-unit is packed unpinned, so the newest installed release in range is created), by a
# release whose meroctl writes them (secrets and `uses` from 1.0.0-rc.31, secrets bound to
# a header or body field from 1.0.0-rc.37; `--guests` from 1.0.0-rc.33). Each is packed when the pinned meroctl can; a case whose package is missing
# then fails to seed, so a pin that cannot pack one says so here.
if "$PACK" --help 2>/dev/null | grep -q -- '--guests'; then
  "$PACK" --wasm "$DEEP/kv-guest.wasm" --package com.calimero.upgrade.kv-guest --app-version 1.0.0 \
    --guests allowed --out "$DEEP/kv-guest-1.0.0.mpk"
else
  echo "note: this meroctl has no --guests (before 1.0.0-rc.33): kv-guest is not packed" >&2
fi
if (
  set -e
  secret_dir="$DEEP/kv-secret-crate"
  rm -rf "$secret_dir" && mkdir -p "$secret_dir"
  cat >"$secret_dir/Cargo.toml" <<'TOML'
[package]
name = "kv-secret"
version = "1.0.0"
edition = "2021"

[package.metadata.calimero]
package = "com.calimero.upgrade.kv-secret"
outbound = ["https://api.example.com/"]

[package.metadata.calimero.secrets.API_KEY]
host = "api.example.com"
paths = ["/"]
header = "x-api-key"
description = "The example API's key"
required = true
TOML
  "$PACK" "$secret_dir" --wasm "$DEEP/kv-secret.wasm" --app-version 1.0.0 --out "$DEEP/kv-secret-1.0.0.mpk"
  unit_dir="$DEEP/kv-unit-crate"
  rm -rf "$unit_dir" && mkdir -p "$unit_dir"
  cat >"$unit_dir/Cargo.toml" <<'TOML'
[package]
name = "kv-unit"
version = "1.0.0"
edition = "2021"

[package.metadata.calimero]
package = "com.calimero.upgrade.kv-unit"

[package.metadata.calimero.uses]
member = { package = "com.calimero.upgrade.kv-secret", range = "^1", mode = "create" }
TOML
  "$PACK" "$unit_dir" --wasm "$DEEP/kv-unit.wasm" --app-version 1.0.0 --out "$DEEP/kv-unit-1.0.0.mpk"
); then :; else
  echo "note: this meroctl cannot pack a release's secrets or uses (before 1.0.0-rc.31): kv-secret and kv-unit are not packed" >&2
fi

# The units examples (a release from 1.0.0-rc.31 on ships them): calendar, planner (its
# release `uses` the calendar with `mode = "create"`) and booking (`mode = "attach"`, with
# the `methods` it calls as the person), packed from each crate's `[package.metadata.calimero]`
# and `calimero.lock` by the release's meroctl, as their authors pack them. Two releases of
# each package a unit upgrade moves: calendar 0.1.1 and planner 0.2.0, whose lock pins it.
if [ -d "$SRC/apps/planner" ] && [ -d "$SRC/apps/booking" ] && [ -d "$SRC/apps/calendar" ]; then
  for app in calendar planner booking; do
    (cd "$SRC" && "$BUILD" "apps/$app" --out "$DEEP/$app.wasm")
  done
  unit() { # DIR WASM VERSION OUT
    "$PACK" "$1" --wasm "$2" --app-version "$3" --out "$DEEP/$4"
  }
  unit "$SRC/apps/calendar" "$DEEP/calendar.wasm" 0.1.0 calendar-0.1.0.mpk
  unit "$SRC/apps/calendar" "$DEEP/calendar.wasm" 0.1.1 calendar-0.1.1.mpk
  unit "$SRC/apps/planner" "$DEEP/planner.wasm" 0.1.0 planner-0.1.0.mpk
  unit "$SRC/apps/booking" "$DEEP/booking.wasm" 0.1.0 booking-0.1.0.mpk
  # planner 0.2.0: the same crate, its lock pinning calendar 0.1.1 (the same ABI).
  next="$DEEP/planner-next"
  rm -rf "$next" && mkdir -p "$next"
  cp -R "$SRC/apps/planner/Cargo.toml" "$SRC/apps/planner/calimero" "$next/"
  sed 's/^version = "0\.1\.0"$/version = "0.1.1"/' "$SRC/apps/planner/calimero.lock" >"$next/calimero.lock"
  grep -q '^version = "0.1.1"$' "$next/calimero.lock" ||
    { echo "build-upgrade-deep-fixtures: planner's calimero.lock does not pin calendar 0.1.0" >&2; exit 1; }
  unit "$next" "$DEEP/planner.wasm" 0.2.0 planner-0.2.0.mpk
else
  echo "build-upgrade-deep-fixtures: $(git -C "$SRC" describe --tags --always) has no units examples (apps/planner, booking, calendar)" >&2
fi
echo "build-upgrade-deep-fixtures: $DEEP from $(git -C "$SRC" describe --tags --always)" >&2
ls -l "$DEEP" >&2
