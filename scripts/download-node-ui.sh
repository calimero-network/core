#!/bin/bash

URL="https://github.com/calimero-network/admin-dashboard/archive/refs/heads/master.zip"

OUTPUT_PATH="../../node-ui/build"

TEMP_DIR="temp_admin_dashboard"

mkdir -p $OUTPUT_PATH

curl -L -o admin-dashboard.zip $URL

unzip -o admin-dashboard.zip -d $TEMP_DIR

mv $TEMP_DIR/admin-dashboard-master/build/* $OUTPUT_PATH

rm admin-dashboard.zip
rm -rf $TEMP_DIR

echo "Static files downloaded and extracted to $OUTPUT_PATH"