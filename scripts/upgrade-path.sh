#!/usr/bin/env bash
# The upgrade path, as ci-checks.yml's `Upgrade path` job runs it: a cluster on the
# release pinned in .github/upgrade-from, rolled onto this tree's merod as mdma rolls a
# Calimero Cloud cluster (crates/node/tests/fault/upgrade_path.py).
#
#   scripts/upgrade-path.sh                 # build this tree, fetch the release, run
#   FROM=1.0.0-rc.15 scripts/upgrade-path.sh
#
# Steps: read the pinned release (a `hard-cutover:` line reports the cutover and stops);
# fetch its merod, meroctl, mero-sign and kv fixture (scripts/fetch-release.sh) and build
# its compute fixture from its tag (scripts/build-release-fixtures.sh); build this tree's
# merod, meroctl, mero-sign and kv (signed as kv 2.0.0, the app upgrade); run the test.
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
scripts/build-release-fixtures.sh "$FROM" "$FROM_DIR"
# The deep cases' components and packages, from the same tag and tools.
scripts/build-upgrade-deep-fixtures.sh "$FROM_DIR"

cargo build --release -p merod -p meroctl -p mero-sign
scripts/install-mero-app-build.sh
mkdir -p "$TARGET/app-components" "$TARGET/upgrade-path-fixtures"
mero-app-build apps/v2-fixtures/kv --out "$TARGET/app-components/kv.wasm"
python3 -I scripts/pack-component.py --wasm "$TARGET/app-components/kv.wasm" \
  --mero-sign "$TARGET/release/mero-sign" \
  --package com.calimero.kv-store --app-version 2.0.0 --out "$TARGET/upgrade-path-fixtures/kv-next.mpk"

status=0
python3 -I crates/node/tests/fault/upgrade_path.py --from-dir "$FROM_DIR" \
  --to-merod "$TARGET/release/merod" --to-meroctl "$TARGET/release/meroctl" \
  --kv-next "$TARGET/upgrade-path-fixtures/kv-next.mpk" \
  --workdir "$TARGET/upgrade-path" --json "$TARGET/upgrade-path.json" || status=$?

python3 -I - "$TARGET/upgrade-path.json" "$FROM" >>"$SUMMARY" <<'EOF' || true
import json, sys
r = json.load(open(sys.argv[1]))
print(f"### Upgrade path: {sys.argv[2]} -> this tree\n")
print("Rollout: " + ", ".join(f"{k} {v:.0f} s" for k, v in r["rollout"].items()) + f"; whole run {r['seconds']:.0f} s\n")
print("| Case | Step | Result | Seconds |\n| --- | --- | --- | ---: |")
for x in r["results"]:
    err = f": {x['error'][:200]}" if x["error"] else ""
    print(f"| {x['case']} | {x['step']} | {x['status']}{err} | {x['seconds']} |")
EOF
exit "$status"
