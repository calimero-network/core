#!/bin/sh
#
# Serve apps on their own port too: add `[server.apps] port` to a stopped node's config.
# Usage: frontend-apps-port.sh <node> <port>

. apps/scaffolding-e2e/scripts/account-api.sh

node="${1:?node}"
port="${2:?port}"

config=$(find "data/${node}" -name config.toml 2>/dev/null | head -1)
[ -n "${config}" ] || fail "no config.toml under data/${node}"
printf '\n[server.apps]\nport = %s\n' "${port}" >> "${config}"
