#!/bin/bash

# Calimero Multi-Node Setup Script
# Usage: ./run-calimero.sh [options]

set -e

# Default values
WASM_FILE=""
FRONTEND_PATH=""
MODE="standalone"
RECREATE_CONTEXT="false"
REMOVE_ORPHANS="false"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print usage
usage() {
    echo -e "${BLUE}Calimero Multi-Node Setup${NC}"
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Required:"
    echo "  -w, --wasm PATH        Path to WASM file (.wasm)"
    echo "  -f, --frontend PATH    Path to frontend source directory"
    echo ""
    echo "Options:"
    echo "  -m, --mode MODE        'standalone' (default) or 'with-auth'"
    echo "  -r, --recreate         Force recreate context (default: false)"
    echo "  --remove-orphans       Remove orphaned containers"
    echo "  -h, --help             Show this help"
    echo ""
    echo "Examples:"
    echo "  $0 -w /path/to/app.wasm -f /path/to/frontend"
    echo "  $0 -w /path/to/app.wasm -f /path/to/frontend -m with-auth"
    echo "  $0 -w /path/to/app.wasm -f /path/to/frontend --recreate"
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -w|--wasm)
            WASM_FILE="$2"
            shift 2
            ;;
        -f|--frontend)
            FRONTEND_PATH="$2"
            shift 2
            ;;
        -m|--mode)
            MODE="$2"
            shift 2
            ;;
        -r|--recreate)
            RECREATE_CONTEXT="true"
            shift
            ;;
        --remove-orphans)
            REMOVE_ORPHANS="true"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo -e "${RED}❌ Unknown option: $1${NC}"
            usage
            exit 1
            ;;
    esac
done

# Validate required arguments
if [[ -z "$WASM_FILE" ]]; then
    echo -e "${RED}❌ Error: WASM file path is required${NC}"
    echo "Use -w or --wasm to specify the path"
    exit 1
fi

if [[ -z "$FRONTEND_PATH" ]]; then
    echo -e "${RED}❌ Error: Frontend path is required${NC}"
    echo "Use -f or --frontend to specify the path"
    exit 1
fi

# Validate file existence
if [[ ! -f "$WASM_FILE" ]]; then
    echo -e "${RED}❌ Error: WASM file not found: $WASM_FILE${NC}"
    exit 1
fi

if [[ ! -d "$FRONTEND_PATH" ]]; then
    echo -e "${RED}❌ Error: Frontend directory not found: $FRONTEND_PATH${NC}"
    exit 1
fi

# Validate mode
if [[ "$MODE" != "standalone" && "$MODE" != "with-auth" ]]; then
    echo -e "${RED}❌ Error: Invalid mode '$MODE'. Use 'standalone' or 'with-auth'${NC}"
    exit 1
fi

# Convert to absolute paths
WASM_FILE=$(realpath "$WASM_FILE")
FRONTEND_PATH=$(realpath "$FRONTEND_PATH")

echo -e "${BLUE}🚀 Starting Calimero Multi-Node Setup${NC}"
echo -e "${GREEN}✅ WASM File: $WASM_FILE${NC}"
echo -e "${GREEN}✅ Frontend: $FRONTEND_PATH${NC}"
echo -e "${GREEN}✅ Mode: $MODE${NC}"
echo -e "${GREEN}✅ Recreate Context: $RECREATE_CONTEXT${NC}"
echo ""

# Build Docker Compose command
COMPOSE_FILES="-f docker-compose.config.yml"
ORPHANS_FLAG=""

if [[ "$MODE" == "with-auth" ]]; then
    COMPOSE_FILES="-f docker-compose.auth.yml -f docker-compose.nodes.yml -f docker-compose.config.yml"
    echo -e "${YELLOW}⚡ Running with authentication (auth + nodes + config)${NC}"
else
    COMPOSE_FILES="-f docker-compose.nodes.yml -f docker-compose.config.yml"
    echo -e "${YELLOW}⚡ Running standalone (nodes + config)${NC}"
fi

if [[ "$REMOVE_ORPHANS" == "true" ]]; then
    ORPHANS_FLAG="--remove-orphans"
fi

# Export environment variables
export WASM_FILE_PATH="$WASM_FILE"
export FRONTEND_SOURCE_PATH="$FRONTEND_PATH"
export CONTEXT_RECREATE="$RECREATE_CONTEXT"

echo -e "${BLUE}🔧 Running Docker Compose...${NC}"
echo ""

# Run Docker Compose
docker-compose $COMPOSE_FILES up $ORPHANS_FLAG
