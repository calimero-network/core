#!/bin/sh
set -e

cd "$(dirname $0)"

./build.sh
./mock_external/build.sh


../context-config/build.sh