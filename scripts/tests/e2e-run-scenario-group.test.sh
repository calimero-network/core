#!/usr/bin/env bash
# Tests for e2e-run-scenario-group.sh, with merobox and docker stubbed out.
#
# A group runs many scenarios on one runner, so the runner itself is what keeps
# them honest: a failure must not skip the rest, every failure must fail the
# group and name its scenario, the fixture-registry check must look only at the
# scenario's own requests, and nothing one scenario leaves behind may reach the
# next. Each stub scenario below breaks one of those.
#
#   bash scripts/tests/e2e-run-scenario-group.test.sh
#
# Stub scenarios and checks are single-quoted on purpose: they expand when they
# run (as a scenario, or under `eval` in check), not here.
# shellcheck disable=SC2016,SC2034
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
runner="$here/../e2e-run-scenario-group.sh"
gates="$here/../e2e-scenario-log-gates.sh"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
repo="$work/repo"
state="$work/state"
bin="$work/bin"
mkdir -p "$repo/apps/demo/workflows" "$repo/scripts" "$state" "$bin"
cp "$gates" "$repo/scripts/"

# docker: containers, networks and volumes are files under $STUB_STATE; the
# fixture registry's log is $STUB_STATE/fetches.
cat >"$bin/docker" <<'EOF'
#!/usr/bin/env bash
s=$STUB_STATE
case "$1 $2" in
  "ps -aq") ls "$s/containers" ;;
  "network ls") ls "$s/networks" ;;
  "volume ls") ls "$s/volumes" ;;
  "network rm") rm -f "$s/networks/$3" ;;
  "volume rm") rm -f "$s/volumes/$4" ;;
  "rm -f") rm -f "$s/containers/$4" ;;
  "inspect -f") echo "/$4" ;;
  logs\ fixture-registry) cat "$s/fetches" 2>/dev/null ;;
  logs\ *) echo "log of $2" ;;
esac
EOF
# merobox: `bootstrap run <file>` runs the scenario file as a script, from the
# app dir, with the runner's environment.
cat >"$bin/merobox" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  bootstrap) exec bash "$3" ;;
  *) exit 0 ;;
esac
EOF
chmod +x "$bin/docker" "$bin/merobox"
mkdir -p "$state/containers" "$state/networks" "$state/volumes"
touch "$state/containers/fixture-registry" "$state/networks/bridge" "$state/volumes/keep"

scenario() { cat >"$repo/apps/demo/workflows/$1.yml"; }
node_log() { printf 'mkdir -p data/container-logs; echo %q >> data/container-logs/node-1.log\n' "$1"; }
fetch='echo "GET /artifacts/demo/1.0.0/demo.mpk" >> "$STUB_STATE/fetches"'
# The isolation probe: a fresh scenario finds no data, no stray container, no stray file.
clean_start='[ ! -e data ] && [ ! -e "$STUB_STATE/containers/stray" ] && [ ! -e ../../stray.txt ] || { echo "DIRTY START"; exit 3; }'

scenario pass <<EOF
$clean_start
$fetch
$(node_log "ordinary line")
echo snapshot > "\$MEROBOX_PRE_RESTART_LOG_DIR/pre-restart-node-1.log"
EOF
scenario fails <<EOF
$clean_start
$fetch
$(node_log "about to fail")
exit 1
EOF
# Passes on its own, but the registry served nothing while it ran; the
# scenarios before it did fetch, so a check of the whole log would wave it through.
scenario no-fetch <<EOF
$clean_start
$(node_log "fetched nothing")
EOF
scenario no-fetch-needed <<EOF
$clean_start
$(node_log "governance only")
EOF
scenario diverges <<EOF
$clean_start
$fetch
$(node_log "WARN unified_projection_divergence plane=\"membership\"")
EOF
# Leaves a container, a network, a volume, node data under a prefix nuke does
# not know, a stray untracked file and an edited tracked file; the next
# scenario's clean_start fails if any survives.
scenario litters <<EOF
$clean_start
$fetch
mkdir -p data/e2e-node-1 && echo state > data/e2e-node-1/db
touch "\$STUB_STATE/containers/stray" "\$STUB_STATE/networks/nat-lan" "\$STUB_STATE/volumes/auth"
echo stray > ../../stray.txt
echo edited >> ../../tracked.txt
$(node_log "littered")
EOF
# Killed by its timeout before merobox could persist any log.
scenario hangs <<EOF
$clean_start
$fetch
touch "\$STUB_STATE/containers/hung-node"
sleep 30
EOF
scenario last <<EOF
$clean_start
grep -q edited ../../tracked.txt && { echo "tracked file still edited"; exit 4; }
$fetch
$(node_log "last one")
EOF

echo original > "$repo/tracked.txt"
git -C "$repo" init -q
git -C "$repo" add tracked.txt scripts apps
git -C "$repo" -c user.email=t@t -c user.name=t commit -qm init
# Downloaded before the first scenario, so teardown must leave it alone.
mkdir -p "$repo/dist" && echo bundle >"$repo/dist/demo-1.0.0.mpk"

entry() {
  printf '{"workflow":"%s","file":"workflows/%s.yml","app":"demo","image":"merod:local","registry_fetch":"%s","timeout_seconds":%s}' \
    "$1" "$1" "${2:-true}" "${3:-60}"
}
json="[$(entry pass),$(entry fails),$(entry no-fetch),$(entry no-fetch-needed false),$(entry diverges),$(entry litters),$(entry hangs true 2),$(entry last)]"

out="$work/out.txt"
summary="$work/summary.md"
(
  cd "$repo" &&
    PATH="$bin:$PATH" STUB_STATE="$state" GITHUB_WORKSPACE="$repo" GITHUB_STEP_SUMMARY="$summary" \
      GROUP=test-01 SCENARIOS_JSON="$json" bash "$runner"
) >"$out" 2>&1
status=$?

failed=0
check() {
  if eval "$2"; then
    echo "ok   $1"
  else
    echo "FAIL $1"
    failed=1
  fi
}
row() { grep -E "^\| [0-9]+ \| $1 \| $2" "$summary" >/dev/null; }

check "the group fails when any scenario fails" '[ "$status" -eq 1 ]'
check "every scenario ran and has a summary line" '[ "$(grep -cE "^\| [0-9]+ \|" "$summary")" -eq 8 ]'
check "a scenario after a failure still runs and passes" 'row last pass'
check "a passing scenario passes" 'row pass pass'
check "a failing scenario fails" 'row fails "\*\*FAIL\*\*"'
check "a scenario the registry served nothing during fails, despite earlier fetches" \
  'row no-fetch "\*\*FAIL\*\*" && grep -q "no-fetch fetched nothing from fixture-registry" "$out"'
check "registry_fetch false is exempt from the fetch check" 'row no-fetch-needed pass'
check "a node-log gate fails its scenario" 'row diverges "\*\*FAIL\*\*.*node-log gate"'
check "a hung scenario is killed by its timeout and fails" 'row hangs "\*\*FAIL\*\*.*timed out"'
check "a littering scenario still passes, and the next starts clean" 'row litters pass && ! grep -q "DIRTY START" "$out"'
check "each failure is annotated with its scenario" \
  'for n in fails no-fetch diverges hangs; do grep -q "::error title=e2e scenario $n failed::" "$out" || exit 1; done'
check "the final error names every failed scenario" \
  'grep -q "4 of 8 scenario(s) failed: fails no-fetch diverges hangs" "$out"'
check "node logs land in a per-scenario dir, prefixed with the scenario" \
  '[ -f "$repo/docker-logs/pass/pass-node-1.log" ] && [ -f "$repo/docker-logs/last/last-node-1.log" ]'
check "a scenario never carries another one's logs" \
  '! grep -rq "about to fail" "$repo/docker-logs/no-fetch" "$repo/docker-logs/last"'
check "pre-restart snapshots go to the scenario's own dir" '[ -f "$repo/docker-logs/pass/pre-restart-node-1.log" ]'
check "a killed scenario's container logs are taken from docker" \
  'grep -qx "log of hung-node" "$repo/docker-logs/hangs/hangs-hung-node.log"'
check "teardown removed what the litterer left" \
  '[ ! -e "$state/containers/stray" ] && [ ! -e "$state/networks/nat-lan" ] && [ ! -e "$state/volumes/auth" ] && [ ! -e "$repo/stray.txt" ] && [ ! -e "$repo/apps/demo/data" ]'
check "teardown kept what was there before the first scenario" \
  '[ -e "$state/containers/fixture-registry" ] && [ -e "$state/networks/bridge" ] && [ -e "$state/volumes/keep" ] && [ -f "$repo/dist/demo-1.0.0.mpk" ] && grep -qx original "$repo/tracked.txt"'

refuse_out=$(cd "$repo" && PATH="$bin:$PATH" STUB_STATE="$state" GITHUB_WORKSPACE="$repo" GROUP=empty SCENARIOS_JSON='[]' bash "$runner" 2>&1)
refuse=$?
check "an empty group is refused, not passed" '[ "$refuse" -eq 1 ] && grep -q "was given no scenarios" <<<"$refuse_out"'

if [ "$failed" -ne 0 ]; then
  echo
  echo "--- runner output ---"
  cat "$out"
  echo "--- summary ---"
  cat "$summary"
  exit 1
fi
echo
echo "all cases pass"
