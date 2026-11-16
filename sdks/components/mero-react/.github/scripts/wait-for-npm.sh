#!/usr/bin/env bash
#
# Wait until npm serves every <name>=<version> given, so a bump that runs right
# after a publish does not resolve against a registry that has not caught up.
#
#   wait-for-npm.sh @calimero-network/mero-js=26.4.0
#
# Exits 0 once every version resolves, 1 if one still does not after the wait.

set -euo pipefail

TRIES=${WAIT_TRIES:-60}   # 60 tries of 10s: npm has taken over a minute to serve a fresh publish
SLEEP_S=${WAIT_SLEEP_S:-10}

for spec in "$@"; do
  name=${spec%=*}
  version=${spec##*=}
  for ((try = 1; ; try++)); do
    served=$(npm view "$name@$version" version --prefer-online 2>/dev/null || true)
    if [ "$served" = "$version" ]; then
      echo "npm serves $name@$version"
      break
    fi
    if [ "$try" -ge "$TRIES" ]; then
      echo "::error::npm still does not serve $name@$version after $((TRIES * SLEEP_S))s"
      exit 1
    fi
    sleep "$SLEEP_S"
  done
done
