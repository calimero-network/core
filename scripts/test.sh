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

# Prepare apps
./build-all-apps.sh

# Prepare contracts
../contracts/registry/build.sh
../contracts/context-config/build.sh
../contracts/proxy-lib/build-test-deps.sh

# Run all tests
if [ "$mode" = "all" ]; then
  echo "Running all tests..."
  cargo +nightly test
  exit 0
fi

# Run tests for changed files
echo "The following Rust files have changed:" "${changed_files[@]}"
# Find affected crates
changed_crates=()
# List of crates defined in workspace
all_crates=($(awk '/\[workspace\]/,/^\s*$/' ../Cargo.toml | grep -E 'members' -A 100 | grep -Eo '"[^"]*"' | tr -d '"' | sed 's|^\./||'))
echo "All workspace crates" "${all_crates[@]}"

for file in "${changed_files[@]}"; do
    # Extract the crate name by stripping the path to get the crate folder
    crate_name=$(echo "$file" | sed -E 's|^(crates/[^/]+)/.*|\1|')
    changed_crates+=("$crate_name")
done
echo "Detected crates from changed files" "${changed_crates[@]}"

# Remove duplicates
unique_changed_crates=()
for item in "${changed_crates[@]}"; do
    # Check if item is already in unique_array
    duplicate=false
    for unique_item in "${unique_changed_crates[@]}"; do
        if [[ "$item" == "$unique_item" ]]; then
            duplicate=true
            break
        fi
    done
    # If item is not a duplicate, add it to unique_array
    if ! $duplicate; then
        unique_changed_crates+=("$item")
    fi
done
echo "Unique crates from changed files" "${unique_changed_crates[@]}"

# array of dependencies in format crates/crate
dep_arr=()
for changed_crate in "${unique_changed_crates[@]}"; do
    # Replace "crates/" with "calimero-" to find dependencies as dependencies are imported as calimero-name from crates/name
    # e.g. In list I have crates/node-primitives but as dependency it is calimero-node-primitives.
    calimero_package_name+=("${changed_crate/crates\//calimero-}")
    # get list of all dependencies from the crate including external dependencies
    dependencies=($(cargo metadata --format-version=1 --no-deps | jq -r --arg CRATE "$calimero_package_name" '
        .packages[] |
        select(.dependencies | any(.name == $CRATE)) |
        .name
    '))

    for dep in "${dependencies[@]}"; do
        # Compare dependency with list of crates from workspace to skip external dependencies
        calimero_dep_crate_path="${dep/calimero-/crates/}"
        for crate in "${all_crates[@]}"; do
            if [[ "$crate" == "$calimero_dep_crate_path" ]]; then
                echo "Found matching dependency: $calimero_dep_crate_path"
                dep_arr+=("$calimero_dep_crate_path")
            fi
        done
    done
done
echo "Dependencies array:" "${dep_arr[@]}"

# Merge crates from changed file and their dependencies
for item in "${dep_arr[@]}"; do
    if [[ -z "${seen[$item]}" ]]; then
        seen[$item]=1 # Mark the item as seen
        unique_changed_crates+=("$item")
    fi
done

# Remove duplicates
crates_to_test=()
for item in "${unique_changed_crates[@]}"; do
    # Check if item is already in unique_array
    duplicate=false
    for unique_item in "${crates_to_test[@]}"; do
        if [[ "$item" == "$unique_item" ]]; then
            duplicate=true
            break
        fi
    done
    # If item is not a duplicate, add it to unique_array
    if ! $duplicate; then
        crates_to_test+=("$item")
    fi
done

# Install the nightly toolchain
rustup toolchain install nightly

# Run tests for each module and its dependencies
echo "Running tests for affected modules and their dependencies..."
echo "Crates to run tests:" "${crates_to_test[@]}"
for crate in "${crates_to_test[@]}"; do
    # Convert from folder name to package name
    crate_package_name=("${crate/crates\//calimero-}")
    if [[  "$crate_package_name" == "calimero-merod" ]]; then
        echo "Testing crate merod"
        cargo +nightly test -p "merod"
    elif [[ "$crate_package_name" == "calimero-meroctl" ]]; then
        echo "Testing crate meroctl"
        cargo +nightly test -p "meroctl"
    else
        echo "Testing crate $crate_package_name"
        cargo +nightly test -p "$crate_package_name"
    fi
done
