#!/bin/bash

# Exit immediately if a command exits with a non-zero status.
set -e

BUILD_SCRIPTS=(
    "./apps/gen-ext/build.sh"
    "./apps/kv-store/build.sh"
    "./apps/only-peers/build.sh"
)

run_script() {
    local script="$1"
    echo "Executing $script..."

    # Check if the build.sh script exists and is executable
    if [ -f "$script" ] && [ -x "$script" ]; then
        "$script"
        echo "$script executed successfully."
    else
        echo "Error: $script does not exist or is not executable."
        exit 1
    fi
}

# Iterate over each script in the array and run it in the background
for script in "${BUILD_SCRIPTS[@]}"; do
    run_script "$script" &
done

# Wait for all background jobs to finish
wait