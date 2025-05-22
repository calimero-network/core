#!/bin/bash

# Exit immediately if a command exits with a non-zero status.
set -ex

BUILD_SCRIPTS=(
    "apps/kv-store/build.sh"
)

run_script() {
    local script="$1"

    # Check if the build.sh script exists and is executable
    if [ -f "$script" ] && [ -x "$script" ]; then
        if "$script"; then
            echo "$script executed successfully."
        else
            echo "Error: $script failed."
            exit 1
        fi
    else
        echo "Error: $script does not exist or is not executable."
        exit 1
    fi
}

# Iterate over each script in the array and run them synchronously
for script in "${BUILD_SCRIPTS[@]}"; do
    run_script "$script"
done
