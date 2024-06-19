#!/usr/bin/env bash

set -o pipefail

# Check if an argument was provided
if [ $# -eq 0 ]; then
  echo "Please provide the number of local nodes argument."
  exit 1
fi

# Get the first command line argument
N=$1

cargo build --bin calimero-node

# Iterate in a loop N times
for ((i = 1; i <= N; i++)); do
  node_home="$HOME/.calimero/node$i"
  echo "\x1b[1;36m(i)\x1b[39m Initializing Node $i at \x1b[33m$node_home\x1b[0m"
  rm -rf "$node_home"
  mkdir -p "$node_home"
  ./target/debug/calimero-node --home "$node_home" \
      init --swarm-port $((2427 + $i)) --server-port $((2527 + $i)) \
    | sed 's/^/ \x1b[1;36m|\x1b[0m  /'
  if [ $? -ne 0 ]; then
    echo " \x1b[1;31m✖\x1b[39m \x1b[33mNode $i\x1b[39m initialization failed, ensure that the node is not already running.\x1b[0m"
    exit 1
  fi
  echo " \x1b[1;32m✔\x1b[39m \x1b[33mNode $i\x1b[39m initialized.\x1b[0m"
done
