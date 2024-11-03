#!/bin/bash

# Exit immediately if a command exits with a non-zero status.
set -ex

cd "$(dirname $0)"

# Check for valid arguments
if [ "$#" -eq 0 ]; then
  echo "No arguments provided. Use --all or --local followed by files."
  exit 1
fi

# Initialize variables
mode=""
changed_files=()

# Parse arguments
for arg in "$@"; do
  case $arg in
    --all)
      mode="all"
      ;;
    --local)
      mode="local"
      ;;
    *)
      if [ -n "$mode" ]; then
        changed_files+=("$arg")
      fi
      ;;
  esac
done

# Check if neither --all nor --local was provided
if [ -z "$mode" ]; then
  echo "Either --all or --local must be provided."
  exit 1
fi

# Check for changed Rust files if local is specified
if [ "$mode" = "local" ] && [ -z "$changed_files" ]; then
  echo "No files provided for local mode. Either no changes were made or the files were not provided."
  exit 0
fi

# prepare apps
./build-all-apps.sh

# Prepare contracts
../contracts/registry/build.sh
../contracts/context-config/build.sh
../contracts/proxy-lib/build-test-deps.sh

# Handle the cases based on the mode
if [ "$mode" = "all" ]; then
  echo "Running all tests..."
  cargo +nightly test
  exit 0
fi

# # Step 1: Find changed files
echo "The following Rust files have changed:" $changed_files

# Step 2: Find affected modules
modules_to_test=""

# Initialize an array to hold matched crates
matched_crates=()
all_crates=($(awk '/\[workspace\]/,/^\s*$/' ../Cargo.toml | grep -E 'members' -A 100 | grep -Eo '"[^"]*"' | tr -d '"' | sed 's|^\./||'))

# Loop through each changed file
for file in "${changed_files[@]}"; do
    # Extract the crate name by stripping the path to get the crate folder
    crate_name=$(echo "$file" | sed -E 's|^(crates/[^/]+)/.*|\1|')
    # Check if the crate exists in the list of crates
    for crate in "${all_crates[@]}"; do
        if [[ "$crate" == "$crate_name" ]]; then
            matched_crates+=("$crate")
        fi
    done
done

echo $matched_crates

calimero_package_names=()
# Loop through each element in the original array
for item in "${matched_crates[@]}"; do
    echo $item
    # Replace "crates/" with "calimero-" and add to new_names array
    calimero_package_names+=("${item/crates\//calimero-}")
done

dep_arr=()

#for each crate from changed file find his dependencies
for calimero_package_name in "${calimero_package_names[@]}"; do
    echo $calimero_package_name
    # Initialize an array to hold the matched dependencies
    for matched_crate in "${matched_crates[@]}"; do
        dependencies=($(cargo metadata --format-version=1 --no-deps | jq -r --arg CRATE "$calimero_package_name" '
            .packages[] |
            select(.dependencies | any(.name == $CRATE)) |
            .name
        '))
        # e.g. calimero-node-primitives. In list I have crates/node-primitives
        # Loop through each dependency
        for dep in "${dependencies[@]}"; do
            # Create the full crate path
            echo "Checking dependency: $dep"
            #replace crates with calimero-
            calimero_dep_crate_path="${dep/calimero-/crates/}"
            for crate in "${all_crates[@]}"; do
                if [[ "$crate" == "$calimero_dep_crate_path" ]]; then
                    echo "Found matching dependency: $calimero_dep_crate_path"
                    dep_arr+=("$calimero_dep_crate_path")
                fi
            done
        done
    done
done

crates_to_test=()
seen=()
# Loop through the calimero_package_names array
for item in "${calimero_package_names[@]}"; do
    if [[ -z "${seen[$item]}" ]]; then
        seen[$item]=1 # Mark the item as seen
        crates_to_test+=("$item")
    fi
done

# Loop through the dep_arr array
for item in "${dep_arr[@]}"; do
    if [[ -z "${seen[$item]}" ]]; then
        seen[$item]=1
        crates_to_test+=("$item")
    fi
done

# Run tests for each module and its dependencies
echo "Running tests for affected modules and their dependencies..."
# Install the nightly toolchain
rustup toolchain install nightly

#Test all crates from changed files
for crate in "${crates_to_test[@]}"; do
    if [[  "$crate" == "calimero-merod" ]]; then
        echo "Testing crate merod"
        cargo +nightly test -p "merod"
    elif [[ "$crate" == "calimero-meroctl" ]]; then
        echo "Testing crate meroctl"
        cargo +nightly test -p "meroctl"
    else
        echo "Testing crate $crate"
        cargo +nightly test -p "$crate"
    fi
done
