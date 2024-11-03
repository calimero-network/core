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
changed_rust_files=""

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
        changed_files="$changed_files $arg"
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

for file in $changed_files; do
    echo $file
    # Convert file paths to module names (strip src/ and .rs)
    module_name=$(echo "$file" | sed -e 's/src\///' -e 's/\.rs//')
    modules_to_test="$modules_to_test $module_name"
done

# Step 3: Run tests for each module and its dependencies
echo "Running tests for affected modules and their dependencies..."

for module in $modules_to_test; do
    echo "Testing module $module and its dependencies"
    cargo +nightly test $module
done