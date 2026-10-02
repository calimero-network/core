#!/usr/bin/env bash
# Runs one group of e2e merobox scenarios on this runner, one after another.
#
#   SCENARIOS_JSON='[{"workflow":..,"file":..,"app":..,"image":..,
#                     "registry_fetch":..,"timeout_seconds":..}, ...]' \
#   GROUP=local-01 scripts/e2e-run-scenario-group.sh
#
# The groups come from scripts/e2e-scenario-groups.py; the workflow has already
# loaded the image, served the bundles and installed merobox, once for all of
# them. For each scenario this:
#
#   1. runs it from apps/<app> with `merobox bootstrap run --e2e-mode`, killed
#      after its timeout_seconds so a hung one fails alone;
#   2. on an http-registry image, fails it unless the fixture registry served a
#      bundle during it (unless registry_fetch is "false");
#   3. copies its container logs to docker-logs/<workflow>/<workflow>-<node>.log;
#   4. tears everything down to the state before the first scenario: merobox
#      stop and nuke, then every container, network and volume, and every
#      untracked path, that was not there before the first scenario ran;
#   5. holds its logs to scripts/e2e-scenario-log-gates.sh.
#
# A failure never stops the group: every scenario runs, each gets a line in the
# step summary and an error annotation naming it, and the script exits 1 at the
# end if any failed.
set -uo pipefail
export LC_ALL=C

group=${GROUP:?GROUP is not set}
scenarios=${SCENARIOS_JSON:?SCENARIOS_JSON is not set}
root=${GITHUB_WORKSPACE:-$(git rev-parse --show-toplevel)}
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
logroot="$root/docker-logs"
gates="$root/scripts/e2e-scenario-log-gates.sh"
# What the fixture registry logs for every bundle it serves.
readonly FETCH_LINE='GET /artifacts/'

count=$(jq 'length' <<<"$scenarios")
if ! [ "$count" -gt 0 ] 2>/dev/null; then
  echo "::error::group ${group} was given no scenarios"
  exit 1
fi

# Everything a scenario may leave behind is measured against this, so the
# fixture registry and the downloaded apps, bundles and fixtures survive.
containers_now() { docker ps -aq --no-trunc | sort; }
networks_now() { docker network ls -q --no-trunc | sort; }
volumes_now() { docker volume ls -q | sort; }
# docker-logs/ is this script's own output.
untracked_now() {
  local listed
  listed=$(git -C "$root" ls-files --others --directory) || return 1
  grep -v '^docker-logs/' <<<"$listed" | sort || true
}
modified_now() { git -C "$root" diff --name-only | sort; }
# A baseline that failed to read would read as "nothing was here", and teardown
# would then delete the apps, bundles and fixture registry every later scenario
# needs, so it is a hard stop rather than a best effort.
if ! base_containers=$(containers_now) || ! base_networks=$(networks_now) \
  || ! base_volumes=$(volumes_now) || ! base_untracked=$(untracked_now) \
  || ! base_modified=$(modified_now); then
  echo "::error::${group}: could not record the runner's state before the first scenario"
  exit 1
fi
added() { comm -13 <(printf '%s\n' "$1") <(printf '%s\n' "$2") | grep -v '^$' || true; }

fixture_fetches() { docker logs fixture-registry 2>&1 | grep -c "$FETCH_LINE" || true; }

# Data dirs are written by root inside the containers.
remove_path() { rm -rf "$1" 2>/dev/null || sudo rm -rf "$1"; }

# Prints what it could not remove; empty output means the runner is back to
# its pre-scenario state.
teardown() {
  local app_dir=$1 id path left=""
  (cd "$app_dir" && merobox stop --all) || true
  (cd "$app_dir" && merobox nuke --force) || true

  for id in $(added "$base_containers" "$(containers_now)"); do
    docker rm -f -v "$id" >/dev/null 2>&1 || true
  done
  for id in $(added "$base_networks" "$(networks_now)"); do
    docker network rm "$id" >/dev/null 2>&1 || true
  done
  for id in $(added "$base_volumes" "$(volumes_now)"); do
    docker volume rm -f "$id" >/dev/null 2>&1 || true
  done
  # nuke deletes only data/ dirs named calimero-node-*, prop-* or proposal-*; a
  # scenario with any other node prefix would hand its state to the next one.
  remove_path "$app_dir/data"
  while IFS= read -r path; do
    [ -n "$path" ] && remove_path "$root/$path"
  done < <(added "$base_untracked" "$(untracked_now)")
  while IFS= read -r path; do
    [ -n "$path" ] && git -C "$root" checkout -q -- "$path"
  done < <(added "$base_modified" "$(modified_now)")

  [ -z "$(added "$base_containers" "$(containers_now)")" ] || left+=" containers"
  [ -z "$(added "$base_networks" "$(networks_now)")" ] || left+=" networks"
  [ -z "$(added "$base_volumes" "$(volumes_now)")" ] || left+=" volumes"
  [ -z "$(added "$base_untracked" "$(untracked_now)")" ] || left+=" files"
  [ -z "$(added "$base_modified" "$(modified_now)")" ] || left+=" tracked-files"
  echo "${left# }"
}

mkdir -p "$logroot"
{
  echo "### ${group}: ${count} scenario(s)"
  echo ""
  echo "| # | scenario | result | took | why |"
  echo "| ---: | --- | --- | ---: | --- |"
} >>"$summary"

failed=()
ran=0
for ((i = 0; i < count; i++)); do
  entry=$(jq -c ".[$i]" <<<"$scenarios")
  name=$(jq -er '.workflow' <<<"$entry")
  file=$(jq -er '.file' <<<"$entry")
  app=$(jq -er '.app' <<<"$entry")
  image=$(jq -er '.image' <<<"$entry")
  registry_fetch=$(jq -er '.registry_fetch' <<<"$entry")
  limit=$(jq -er '.timeout_seconds' <<<"$entry")
  app_dir="$root/apps/$app"
  logdir="$logroot/$name"
  mkdir -p "$logdir"
  why=()

  echo "::group::[$((i + 1))/${count}] ${name} (apps/${app}/${file}, ${image})"
  started=$(date +%s)
  if [ "$image" != merod:local-dht ]; then
    fetched_before=$(fixture_fetches)
  fi

  # Single attempt, fail loud: a retry loop here used to mask cold-start governance flakes. With
  # the three-phase contract in place, a flake now is a real bug worth surfacing.
  # Points merobox's restart_container pre-restart snapshot at this scenario's own log dir;
  # otherwise it writes to a CWD-relative docker-logs/ the uploader never sees.
  (
    cd "$app_dir" &&
      MEROBOX_PRE_RESTART_LOG_DIR="$logdir" timeout --kill-after=60 "$limit" \
        merobox bootstrap run "$file" --image "$image" --e2e-mode
  )
  status=$?
  case $status in
    0) ;;
    124 | 137) why+=("timed out after ${limit}s") ;;
    *) why+=("merobox exited ${status}") ;;
  esac

  # A green http scenario proves nothing on its own: the node logs an
  # acquisition either way, so ask the fixture whether it served one during
  # this scenario (its log spans the whole group).
  if [ "$image" != merod:local-dht ] && [ "$registry_fetch" != false ] \
    && [ "$(fixture_fetches)" -le "$fetched_before" ]; then
    echo "::error::${name} fetched nothing from fixture-registry; it did not use the http registry"
    why+=("fetched nothing from the fixture registry")
  fi

  # merobox >=0.6.34 persists per-container logs to data/container-logs/<name>.log natively on
  # every bootstrap run. Collected before teardown clears node state; prefixed with the scenario
  # so filenames stay self-describing when the suite-wide jobs merge every group's logs.
  for f in "$app_dir"/data/container-logs/*.log; do
    [ -e "$f" ] || continue
    cp -f "$f" "$logdir/${name}-$(basename "$f")"
  done
  # A killed merobox never got to write them; take them from the containers instead.
  if [ "$status" -eq 124 ] || [ "$status" -eq 137 ]; then
    for id in $(added "$base_containers" "$(containers_now)"); do
      cname=$(docker inspect -f '{{.Name}}' "$id" 2>/dev/null | sed 's#^/##')
      [ -n "$cname" ] && [ ! -e "$logdir/${name}-${cname}.log" ] || continue
      docker logs "$id" >"$logdir/${name}-${cname}.log" 2>&1 || true
    done
  fi

  left=$(teardown "$app_dir")
  if [ -n "$left" ]; then
    echo "::error::${name} left${left:+ }${left} behind that could not be removed; later scenarios in ${group} may not start clean"
    why+=("teardown left ${left} behind")
  fi

  if ! "$gates" "$logdir" "$name"; then
    why+=("a node-log gate failed")
  fi

  took=$(($(date +%s) - started))
  ran=$((ran + 1))
  echo "::endgroup::"

  if [ "${#why[@]}" -eq 0 ]; then
    echo "PASS ${name} (${took}s)"
    echo "| $((i + 1)) | ${name} | pass | ${took}s | |" >>"$summary"
  else
    reason=$(printf '%s; ' "${why[@]}")
    reason=${reason%; }
    failed+=("$name")
    echo "::error title=e2e scenario ${name} failed::${name} (apps/${app}/${file}): ${reason}"
    echo "| $((i + 1)) | ${name} | **FAIL** | ${took}s | ${reason} |" >>"$summary"
  fi
done

echo "" >>"$summary"
if [ "$ran" -ne "$count" ]; then
  echo "::error::${group} ran ${ran} of its ${count} scenarios"
  exit 1
fi
if [ "${#failed[@]}" -gt 0 ]; then
  echo "::error::${group}: ${#failed[@]} of ${count} scenario(s) failed: ${failed[*]}"
  exit 1
fi
echo "${group}: all ${count} scenario(s) passed"
