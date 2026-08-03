#!/bin/sh
# TEMPORARY diagnostic: print the host-side layout merobox produced for a node,
# plus what its container reports. Removed once the export path is understood.
set -u
container="$1"
echo "=== cwd: $(pwd)"
echo "=== ls data/:"; ls -la data/ 2>&1 | head -20
echo "=== ls data/${container}/:"; ls -la "data/${container}/" 2>&1 | head -20
echo "=== ls data/${container}/${container}/:"; ls -la "data/${container}/${container}/" 2>&1 | head -20
echo "=== find any config.toml under data/:"; find data -name config.toml 2>/dev/null | head -10
echo "=== docker ps -a (calimero):"; docker ps -a --filter "label=calimero.node=true" --format '{{.Names}} {{.Status}} {{.Image}}' 2>&1 | head -10
echo "=== inspect mounts for ${container}:"; docker inspect -f '{{range .Mounts}}{{.Source}} -> {{.Destination}}{{"\n"}}{{end}}' "${container}" 2>&1 | head -5
exit 0
