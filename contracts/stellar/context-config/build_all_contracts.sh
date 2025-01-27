#!/bin/sh
set -e

cd "$(dirname $0)"

./build.sh
./mock_proxy/build.sh
