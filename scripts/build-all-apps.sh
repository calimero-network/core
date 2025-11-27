#!/bin/bash

# Exit immediately if a command exits with a non-zero status.
set -ex

BUILD_SCRIPTS=(
    "apps/kv-store/build.sh"
    "apps/kv-store-with-handlers/build.sh"
    "apps/kv-store-init/build.sh"
    "apps/kv-store-with-user-and-frozen-storage/build.sh"
    "apps/collaborative-editor/build.sh"
    "apps/nested-crdt-test/build.sh"
    "apps/team-metrics-macro/build.sh"
    "apps/team-metrics-custom/build.sh"
    "apps/blobs/build.sh"
    "apps/private_data/build.sh"
    "apps/xcall-example/build.sh"
    "apps/access-control/build.sh"
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
