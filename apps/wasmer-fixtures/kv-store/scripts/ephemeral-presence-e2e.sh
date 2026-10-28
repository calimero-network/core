#!/bin/sh
# Wrapper: merobox runs script steps through `sh`, so the .js needs an explicit
# node. The assertions live in the .js because presence can only be observed by
# subscribing to the event stream (there is no read endpoint), and a WebSocket
# client in shell is not a reasonable thing to write.
exec node "$(dirname "$0")/ephemeral-presence-e2e.js" "$@"
