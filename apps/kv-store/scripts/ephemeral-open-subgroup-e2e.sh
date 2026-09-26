#!/bin/sh
# Wrapper: merobox runs script steps through `sh`, so the .js needs an explicit node.
exec node "$(dirname "$0")/ephemeral-open-subgroup-e2e.js" "$@"
