#!/bin/bash

show_usage() {
    echo "Usage: $0 delete-and-start || start"
    exit 1
}

current_dir=$(pwd)

if [ $# -ne 1 ]; then
    show_usage
fi

case "$1" in
    delete-and-start)
        rm -rf "$current_dir/data"
        mkdir -p "$current_dir/data"
        osascript <<EOF
tell application "Terminal"
    activate
    do script "cd '$current_dir' && cargo run -p calimero-node -- --home data/coordinator init --server-port 2427 --swarm-port 2527 && cargo run -p calimero-node -- --home data/coordinator run --node-type coordinator"
    delay 1
    tell application "System Events" to keystroke "t" using command down
    delay 1
    do script "cd '$current_dir' && cargo run -p calimero-node -- --home data/node1 init --server-port 2428 --swarm-port 2528 && cargo run -p calimero-node -- --home data/node1 run" in selected tab of the front window
    delay 1
    tell application "System Events" to keystroke "t" using command down
    delay 1
    do script "cd '$current_dir' && cargo run -p calimero-node -- --home data/node2 init --server-port 2420 --swarm-port 2529 && cargo run -p calimero-node -- --home data/node2 run" in selected tab of the front window
end tell
EOF
        ;;
    start)
        osascript <<EOF
tell application "Terminal"
    activate
    do script "cd '$current_dir' && cargo run -p calimero-node -- --home data/coordinator run --node-type coordinator"
    delay 1
    tell application "System Events" to keystroke "t" using command down
    delay 1
    do script "cd '$current_dir' && cargo run -p calimero-node -- --home data/node1 run" in selected tab of the front window
    delay 1
    tell application "System Events" to keystroke "t" using command down
    delay 1
    do script "cd '$current_dir' && cargo run -p calimero-node -- --home data/node2 run" in selected tab of the front window
end tell
EOF
        ;;
    *)
        show_usage
        ;;
esac
