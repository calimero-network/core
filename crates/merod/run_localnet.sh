#!/usr/bin/env bash

set -o pipefail

# Set default NODE_HOME 
: ${NODE_HOME:="$HOME/.calimero"}

# Check if an argument was provided
if [ $# -eq 0 ]; then
  echo "Please provide the number of local nodes argument."
  exit 1
fi

# Get the first command line argument
N=$1

# Array to store PIDs of running nodes
declare -a NODE_PIDS

# Function to cleanup on exit
cleanup() {
    echo -e "\n\x1b[1;36m(i)\x1b[39m Cleaning up..."
    for pid in "${NODE_PIDS[@]}"; do
        if ps -p "$pid" > /dev/null; then
            echo -e " \x1b[1;36m|\x1b[0m Stopping node with PID: $pid"
            kill "$pid" 2>/dev/null || true
        fi
    done
    echo -e " \x1b[1;32m✔\x1b[39m Cleanup complete.\x1b[0m"
}

# Set up trap to ensure cleanup runs on script exit
trap cleanup EXIT

cargo build --bin merod

echo -e "\x1b[1;36m(i)\x1b[39m Starting $N local nodes...\x1b[0m"

# Start each node in the background
for ((i = 1; i <= N; i++)); do
    node_home="$HOME/.calimero/"
    echo -e " \x1b[1;36m|\x1b[0m Starting Node $i..."
    
    # Start the node in the background and capture its PID
    ./target/debug/merod --home "$node_home" --node-name "node$i" run > "$node_home/node.log" 2>&1 &
    NODE_PIDS[$i]=$!
    
    # Wait a bit to ensure the node has time to start
    sleep 2
    
    # Check if the process is still running
    if ! ps -p "${NODE_PIDS[$i]}" > /dev/null; then
        echo -e " \x1b[1;31m✖\x1b[39m \x1b[33mNode $i\x1b[39m failed to start. Check $node_home/node.log for details.\x1b[0m"
        exit 1
    fi
    
    echo -e " \x1b[1;32m✔\x1b[39m \x1b[33mNode $i\x1b[39m started with PID: ${NODE_PIDS[$i]}\x1b[0m"
done

echo -e "\x1b[1;32m✔\x1b[39m All $N nodes started successfully.\x1b[0m"
echo -e "\x1b[1;36m(i)\x1b[39m Press Ctrl+C to stop all nodes.\x1b[0m"

# Keep the script running and wait for Ctrl+C
while true; do
    sleep 1
done 