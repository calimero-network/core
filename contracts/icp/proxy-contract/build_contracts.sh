#!/bin/sh
set -e

cd "$(dirname $0)"

echo "Building proxy contract..."
./build.sh

echo "Building mock ledger contract..."
./mock/ledger/build.sh

echo "Building mock external contract..."
./mock/external/build.sh

echo "Building context-config contract..."
../context-config/build.sh
