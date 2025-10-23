#!/bin/bash

# CLI Commands Test
# Tests the new registry and app registry CLI commands

set -e

echo "🧪 Testing CLI Commands"
echo "======================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test CLI command
test_cli_command() {
    local command=$1
    local description=$2
    
    echo -e "${YELLOW}Testing: ${description}${NC}"
    echo "  Command: $command"
    
    if eval "$command" 2>&1 | grep -q "Usage\|Commands\|Options\|--help"; then
        echo -e "  ${GREEN}✅ PASS${NC}"
    else
        echo -e "  ${RED}❌ FAIL${NC}"
        echo "  Output: $(eval "$command" 2>&1 | head -n 3)"
        return 1
    fi
    echo ""
}

echo "Testing Registry CLI Commands:"
echo ""

# Test registry commands
test_cli_command "./target/debug/meroctl --help" "Main CLI help"
test_cli_command "./target/debug/meroctl registry --help" "Registry command help"
test_cli_command "./target/debug/meroctl registry setup --help" "Registry setup help"
test_cli_command "./target/debug/meroctl registry list --help" "Registry list help"
test_cli_command "./target/debug/meroctl registry remove --help" "Registry remove help"

echo "Testing App Registry CLI Commands:"
echo ""

# Test app registry commands
test_cli_command "./target/debug/meroctl app --help" "App command help"
test_cli_command "./target/debug/meroctl app registry --help" "App registry command help"
test_cli_command "./target/debug/meroctl app registry list --help" "App registry list help"
test_cli_command "./target/debug/meroctl app registry install --help" "App registry install help"
test_cli_command "./target/debug/meroctl app registry update --help" "App registry update help"
test_cli_command "./target/debug/meroctl app registry uninstall --help" "App registry uninstall help"

echo -e "${GREEN}🎉 CLI Commands Test Completed!${NC}"
echo ""
echo "All CLI commands are properly configured and accessible."
echo ""
echo "Available commands:"
echo "  meroctl registry setup local --name dev --port 8082"
echo "  meroctl registry setup remote --name prod --url https://registry.example.com"
echo "  meroctl registry list"
echo "  meroctl registry remove dev"
echo "  meroctl app registry list --registry dev"
echo "  meroctl app registry install my-app --registry dev --version 1.0.0"
echo "  meroctl app registry update my-app --registry dev"
echo "  meroctl app registry uninstall my-app --registry dev"
