#!/usr/bin/env bash
# The upgrade path, as ci-checks.yml's `Upgrade path` job runs it: a cluster on the
# release pinned in .github/upgrade-from, rolled onto this tree's merod as mdma rolls a
# Calimero Cloud cluster (crates/node/tests/fault/upgrade_path.py).
#
#   scripts/upgrade-path.sh                 # build this tree, fetch the release, run
#   FROM=1.0.0-rc.15 scripts/upgrade-path.sh
#   GOVERNANCE=granted scripts/upgrade-path.sh   # the nightly run: owner governance, granted deployments
#
# Steps: read the pinned release (a `hard-cutover:` line reports the cutover and stops);
# fetch its merod, meroctl (and mero-sign, for a release that shipped it) and kv fixture (scripts/fetch-release.sh) and build
# its compute fixture from its tag (scripts/build-release-fixtures.sh); build this tree's
# merod and meroctl, and kv with that meroctl (signed as kv 2.0.0, the app upgrade), in the
# background beside the release's fixtures and the test's seed; the test waits for that build
# before the roll.
# A pinned tag that does not exist yet is reported and not run; a tag whose release
# lacks an artifact fails.
#
# Work: $CARGO_TARGET_DIR (default target)/upgrade-from and /upgrade-path; results in
# target/upgrade-path.json. WORKFLOW_PORT_OFFSET moves its ports (7671-7680).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
cd "$ROOT"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
FROM_DIR="$TARGET/upgrade-from"
SUMMARY="${GITHUB_STEP_SUMMARY:-/dev/null}"

# The coverage matrix in the docs is generated from the deep cases; refuse a stale copy
# before building anything.
python3 -I crates/node/tests/fault/upgrade_deep.py --check-matrix docs/src/content/docs/contribute/testing.mdx
python3 -I crates/node/tests/fault/upgrade_runner_test.py

pinned=$(grep -v '^[[:space:]]*#' .github/upgrade-from | grep -v '^[[:space:]]*$' | head -n 1)
FROM="${FROM:-$pinned}"
case "$FROM" in
  hard-cutover:*)
    msg="The upgrade path is not run: .github/upgrade-from declares ${FROM}. Existing clusters are recreated, not rolled; the next release puts a version back."
    echo "::warning::$msg"
    echo "$msg" >>"$SUMMARY"
    exit 0
    ;;
esac

# The newest published version tag, to say when the pin is behind.
newest=$(git ls-remote --tags --refs "https://github.com/${REPO:-calimero-network/core}.git" | awk -F/ '{print $3}' |
  python3 -I -c '
import re, sys
def key(v):
    m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-rc\.(\d+))?", v)
    return None if not m else (int(m[1]), int(m[2]), int(m[3]), 1 if m[4] is None else 0, int(m[4] or 0))
tags = [t.strip() for t in sys.stdin if key(t.strip())]
print(max(tags, key=key) if tags else "")') || newest=""
if [ -n "$newest" ] && [ "$newest" != "$FROM" ]; then
  echo "::warning::.github/upgrade-from pins $FROM, and $newest is tagged: once its release is published, set the pin to it (CONTRIBUTING.md, Cutting a release)."
fi

rc=0
scripts/fetch-release.sh "$FROM" "$FROM_DIR" || rc=$?
if [ "$rc" = 3 ]; then
  msg="The upgrade path is not run: $FROM, pinned in .github/upgrade-from, is not tagged yet. It runs once the tag exists, and fails if its release then lacks an artifact."
  echo "::warning::$msg"
  echo "$msg" >>"$SUMMARY"
  exit 0
fi
[ "$rc" = 0 ] || exit "$rc"
# This tree's binaries and fixtures build in the background, from here on beside the release's
# fixtures and then the old cluster's boot and seed (which need only the fetched release); the
# test waits for them before the roll.
BUILD_LOG="$TARGET/upgrade-path-build.log"
BUILD_STATUS="$TARGET/upgrade-path-build.status"
RELEASE_FIXTURES_DONE="$TARGET/upgrade-path-release-fixtures.done"
rm -f "$BUILD_STATUS" "$RELEASE_FIXTURES_DONE"

build_this_tree() {
  cargo build --release -p merod -p meroctl
  MEROCTL="$TARGET/release/meroctl"
  mkdir -p "$TARGET/app-components" "$TARGET/upgrade-path-fixtures"
  "$MEROCTL" app build apps/fixture-kv --out "$TARGET/app-components/kv.wasm"
  "$MEROCTL" app pack --wasm "$TARGET/app-components/kv.wasm" \
    --package com.calimero.kv-store --app-version 2.0.0 --out "$TARGET/upgrade-path-fixtures/kv-next.mpk"
  until [ -e "$RELEASE_FIXTURES_DONE" ]; do sleep 1; done
  # The pipeline cases' package, installed on the old release: the compute component the
  # release's own tools built from its tag (build-release-fixtures.sh), so it imports no
  # calimero:app version newer than that release knows, and the release's fanout pipeline.
  # An older release keeps its test fixtures under apps/v2-fixtures/.
  FANOUT="$FROM_DIR/src/apps/fixture-compute/pipelines/fanout.yaml"
  [ -e "$FANOUT" ] || FANOUT="$FROM_DIR/src/apps/v2-fixtures/compute/pipelines/fanout.yaml"
  "$MEROCTL" app pack --compute "$FROM_DIR/compute-compute.wasm" \
    --pipeline "$FANOUT" \
    --package com.calimero.upgrade.pipelines --app-version 1.0.0 --out "$TARGET/upgrade-path-fixtures/pipelines.mpk"
}

# The exit trap leaves the build's exit status where the test looks for it.
( trap 'echo $? >"$BUILD_STATUS"' EXIT; build_this_tree ) >"$BUILD_LOG" 2>&1 &
BUILD_PID=$!
# A failure below ends the script: stop waiting for the fixtures it never made.
trap 'kill "$BUILD_PID" 2>/dev/null || true' EXIT

scripts/build-release-fixtures.sh "$FROM" "$FROM_DIR"
# The deep cases' components and packages, from the same tag and tools.
scripts/build-upgrade-deep-fixtures.sh "$FROM_DIR"
touch "$RELEASE_FIXTURES_DONE"

status=0
python3 -I crates/node/tests/fault/upgrade_path.py --governance "${GOVERNANCE:-open}" --from-dir "$FROM_DIR" \
  --to-merod "$TARGET/release/merod" --to-meroctl "$TARGET/release/meroctl" \
  --kv-next "$TARGET/upgrade-path-fixtures/kv-next.mpk" --pipelines-mpk "$TARGET/upgrade-path-fixtures/pipelines.mpk" \
  --build-status "$BUILD_STATUS" \
  --workdir "$TARGET/upgrade-path" --json "$TARGET/upgrade-path.json" || status=$?

wait "$BUILD_PID" || true
if [ "$(cat "$BUILD_STATUS" 2>/dev/null || echo 1)" = 0 ]; then
  echo "::group::Build of this tree"
  cat "$BUILD_LOG"
  echo "::endgroup::"
else
  echo "::error::The build of this tree failed:"
  cat "$BUILD_LOG"
  status=1
fi

python3 -I - "$TARGET/upgrade-path.json" "$FROM" >>"$SUMMARY" <<'EOF' || true
import json, sys
r = json.load(open(sys.argv[1]))
print(f"### Upgrade path ({r.get('governance', 'open')}): {sys.argv[2]} -> this tree\n")
print("Phases: " + ", ".join(f"{k} {v:.0f} s" for k, v in r.get("phases", {}).items()) + "\n")
print("Rollout: " + ", ".join(f"{k} {v:.0f} s" for k, v in r["rollout"].items()) + f"; whole run {r['seconds']:.0f} s\n")
print("| Case | Step | Result | Seconds |\n| --- | --- | --- | ---: |")
for x in r["results"]:
    err = f": {x['error'][:200]}" if x["error"] else ""
    print(f"| {x['case']} | {x['step']} | {x['status']}{err} | {x['seconds']} |")
EOF
exit "$status"
