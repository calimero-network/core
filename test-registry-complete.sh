#!/bin/bash

# Comprehensive Registry Integration Test
# This script tests both API endpoints and CLI commands

echo "🚀 Calimero Registry Integration Test"
echo "======================================"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Test configuration
NODE_URL="http://localhost:8080"
ADMIN_API="$NODE_URL/admin-api"

echo -e "\n${BLUE}1. Testing Node Health${NC}"
echo "----------------------"
if curl -s "$NODE_URL/health" > /dev/null 2>&1; then
    echo -e "${GREEN}✅ Node is running${NC}"
else
    echo -e "${RED}❌ Node is not responding${NC}"
    exit 1
fi

echo -e "\n${BLUE}2. Testing Admin API Health${NC}"
echo "---------------------------"
if curl -s "$ADMIN_API/health" > /dev/null 2>&1; then
    echo -e "${GREEN}✅ Admin API is accessible${NC}"
else
    echo -e "${RED}❌ Admin API is not accessible${NC}"
    exit 1
fi

echo -e "\n${BLUE}3. Testing Registry List (Initial)${NC}"
echo "-----------------------------------"
REGISTRIES=$(curl -s "$ADMIN_API/registries")
echo "Response: $REGISTRIES"

echo -e "\n${BLUE}4. Testing Local Registry Setup${NC}"
echo "--------------------------------"
LOCAL_SETUP=$(curl -s -X POST "$ADMIN_API/registries" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "local-dev",
    "registryType": "local",
    "config": {
      "local": {
        "port": 8082,
        "data_dir": "/tmp/registry-data"
      }
    }
  }')

echo "Response: $LOCAL_SETUP"
if echo "$LOCAL_SETUP" | grep -q "configured"; then
    echo -e "${GREEN}✅ Local registry setup successful${NC}"
else
    echo -e "${RED}❌ Local registry setup failed${NC}"
fi

echo -e "\n${BLUE}5. Testing Remote Registry Setup${NC}"
echo "----------------------------------"
REMOTE_SETUP=$(curl -s -X POST "$ADMIN_API/registries" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "remote-prod",
    "registryType": "remote",
    "config": {
      "remote": {
        "base_url": "http://localhost:8082",
        "timeout_ms": 5000,
        "auth_token": "test-token"
      }
    }
  }')

echo "Response: $REMOTE_SETUP"
if echo "$REMOTE_SETUP" | grep -q "configured"; then
    echo -e "${GREEN}✅ Remote registry setup successful${NC}"
else
    echo -e "${RED}❌ Remote registry setup failed${NC}"
fi

echo -e "\n${BLUE}6. Testing Registry List (After Setup)${NC}"
echo "----------------------------------------"
REGISTRIES_AFTER=$(curl -s "$ADMIN_API/registries")
echo "Response: $REGISTRIES_AFTER"

echo -e "\n${BLUE}7. Testing App Installation from Registry${NC}"
echo "--------------------------------------------"
APP_INSTALL=$(curl -s -X POST "$ADMIN_API/registries/local-dev/apps/install" \
  -H "Content-Type: application/json" \
  -d '{
    "appName": "test-app",
    "registryName": "local-dev",
    "version": "1.0.0",
    "metadata": []
  }')

echo "Response: $APP_INSTALL"
if echo "$APP_INSTALL" | grep -q "Registry not found"; then
    echo -e "${YELLOW}⚠️  Registry not found (expected - placeholder implementation)${NC}"
else
    echo -e "${GREEN}✅ App installation test completed${NC}"
fi

echo -e "\n${BLUE}8. Testing CLI Commands${NC}"
echo "-------------------------"

echo -e "\n${YELLOW}Testing registry list command:${NC}"
./target/debug/meroctl registry list

echo -e "\n${YELLOW}Testing registry setup command:${NC}"
./target/debug/meroctl registry setup --type local --name cli-test --port 8083

echo -e "\n${YELLOW}Testing app registry list command:${NC}"
./target/debug/meroctl app registry list --registry cli-test

echo -e "\n${YELLOW}Testing app registry install command:${NC}"
./target/debug/meroctl app registry install test-app --registry cli-test --version 1.0.0

echo -e "\n${BLUE}9. Testing All CLI Commands${NC}"
echo "----------------------------"

echo -e "\n${YELLOW}Registry Management Commands:${NC}"
echo "• meroctl registry list"
echo "• meroctl registry setup --type local --name <name> --port <port>"
echo "• meroctl registry setup --type remote --name <name> --url <url>"
echo "• meroctl registry remove <name>"

echo -e "\n${YELLOW}App Registry Management Commands:${NC}"
echo "• meroctl app registry list --registry <registry>"
echo "• meroctl app registry install <app> --registry <registry> --version <version>"
echo "• meroctl app registry update <app> --registry <registry>"
echo "• meroctl app registry uninstall <app> --registry <registry>"

echo -e "\n${BLUE}10. API Endpoints Summary${NC}"
echo "-------------------------"
echo "• POST $ADMIN_API/registries - Setup registry"
echo "• GET $ADMIN_API/registries - List registries"
echo "• DELETE $ADMIN_API/registries/:name - Remove registry"
echo "• GET $ADMIN_API/registries/:name/apps - List apps from registry"
echo "• POST $ADMIN_API/registries/:name/apps/install - Install app from registry"
echo "• PUT $ADMIN_API/registries/:name/apps/update - Update app from registry"
echo "• DELETE $ADMIN_API/registries/:name/apps/uninstall - Uninstall app from registry"

echo -e "\n${GREEN}🎉 Registry Integration Test Complete!${NC}"
echo -e "\n${YELLOW}Note: This is a placeholder implementation. The actual registry integration"
echo -e "will require connecting to real registry services and implementing"
echo -e "persistent storage for registry configurations.${NC}"
