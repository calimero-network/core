#!/bin/sh
# Wrapper: see ephemeral-presence-e2e.sh.
exec node "$(dirname "$0")/ephemeral-ttl-check.js" "$@"
