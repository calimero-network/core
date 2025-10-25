#!/bin/bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MEROD_BIN="${PROJECT_ROOT}/target/debug/merod"
MEROCTL_BIN="${PROJECT_ROOT}/target/debug/meroctl"
RESULTS_DIR="${PROJECT_ROOT}/e2e-tests-merobox/results"

# Default values
PROTOCOL=""
WORKFLOW=""
VERBOSE=""
BUILD_BINARIES=false
BUILD_APPS=false
USE_VENV=true
VENV_DIR="${PROJECT_ROOT}/.venv-merobox"

# Print usage
usage() {
    echo -e "${BLUE}Usage:${NC} $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  -p, --protocol PROTOCOL    Protocol to test (near, icp, ethereum, all, or near-proposals)"
    echo "  -w, --workflow WORKFLOW    Path to workflow YAML file (overrides protocol)"
    echo "  -v, --verbose              Enable verbose output"
    echo "  -b, --build                Build merod and meroctl binaries before testing"
    echo "  -a, --build-apps           Build WASM applications before testing"
    echo "  --no-venv                  Don't use virtual environment (not recommended)"
    echo "  -h, --help                 Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 --protocol near                    # Run NEAR KV store tests"
    echo "  $0 --protocol near --build            # Build binaries, then test"
    echo "  $0 --protocol all --build --build-apps # Build everything, then test all"
    echo "  $0 --workflow path/to/custom.yml      # Run custom workflow"
    echo ""
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -p|--protocol)
            PROTOCOL="$2"
            shift 2
            ;;
        -w|--workflow)
            WORKFLOW="$2"
            shift 2
            ;;
        -v|--verbose)
            VERBOSE="--verbose"
            shift
            ;;
        -b|--build)
            BUILD_BINARIES=true
            shift
            ;;
        -a|--build-apps)
            BUILD_APPS=true
            shift
            ;;
        --no-venv)
            USE_VENV=false
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo -e "${RED}Error: Unknown option $1${NC}"
            usage
            exit 1
            ;;
    esac
done

# Print banner
echo -e "${BLUE}╔═══════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║  Merobox E2E Tests - Local Runner                        ║${NC}"
echo -e "${BLUE}╚═══════════════════════════════════════════════════════════╝${NC}"
echo ""

# Setup virtual environment for merobox
if [ "$USE_VENV" = true ]; then
    echo -e "${BLUE}Setting up Python virtual environment...${NC}"
    
    # Remove existing venv if it exists
    if [ -d "$VENV_DIR" ]; then
        echo -e "${YELLOW}Removing existing virtual environment...${NC}"
        rm -rf "$VENV_DIR"
    fi
    
    # Create fresh virtual environment
    echo -e "${BLUE}Creating virtual environment at $VENV_DIR${NC}"
    if python3 -m venv "$VENV_DIR"; then
        echo -e "${GREEN}✓ Virtual environment created${NC}"
    else
        echo -e "${RED}Error: Failed to create virtual environment${NC}"
        echo -e "${YELLOW}Make sure python3-venv is installed${NC}"
        exit 1
    fi
    
    # Activate virtual environment
    source "$VENV_DIR/bin/activate"
    
    # Upgrade pip
    echo -e "${BLUE}Upgrading pip...${NC}"
    pip install --upgrade pip > /dev/null 2>&1
    
    # Install merobox in the virtual environment
    echo -e "${BLUE}Installing merobox in virtual environment...${NC}"
    if pip install merobox; then
        echo -e "${GREEN}✓ Merobox installed successfully${NC}"
    else
        echo -e "${RED}Error: Failed to install merobox${NC}"
        exit 1
    fi
    
    echo -e "${GREEN}✓ Virtual environment ready${NC}"
    echo ""
fi

# Build binaries if requested
if [ "$BUILD_BINARIES" = true ]; then
    echo -e "${BLUE}Building binaries...${NC}"
    cd "$PROJECT_ROOT"
    if cargo build -p merod -p meroctl; then
        echo -e "${GREEN}✓ Binaries built successfully${NC}"
    else
        echo -e "${RED}Error: Failed to build binaries${NC}"
        exit 1
    fi
    echo ""
fi

# Build apps if requested
if [ "$BUILD_APPS" = true ]; then
    echo -e "${BLUE}Building WASM applications...${NC}"
    cd "$PROJECT_ROOT"
    if ./apps/kv-store/build.sh; then
        echo -e "${GREEN}✓ KV store app built successfully${NC}"
    else
        echo -e "${RED}Error: Failed to build kv-store app${NC}"
        exit 1
    fi
    echo ""
fi

# Verify merobox is installed and working
if [ "$USE_VENV" = false ]; then
    # Only do extensive checks if not using venv (since venv just installed it)
    if ! command -v merobox &> /dev/null; then
        echo -e "${RED}Error: merobox is not installed${NC}"
        echo -e "${YELLOW}Install it with: pip install merobox${NC}"
        echo -e "${YELLOW}Or run this script without --no-venv to use virtual environment${NC}"
        exit 1
    fi
    
    # Test if merobox actually works (catches Python GIL errors)
    if ! merobox --version &> /dev/null; then
        echo -e "${RED}Error: merobox is installed but not working${NC}"
        echo -e "${YELLOW}This is often due to Python environment issues.${NC}"
        echo -e "${YELLOW}Try running without --no-venv flag (recommended)${NC}"
        exit 1
    fi
fi

echo -e "${GREEN}✓ Merobox version:${NC} $(merobox --version)"

# Check if binaries exist
if [ ! -f "$MEROD_BIN" ]; then
    echo -e "${RED}Error: merod binary not found at $MEROD_BIN${NC}"
    echo -e "${YELLOW}Build it with: cargo build -p merod${NC}"
    echo -e "${YELLOW}Or run this script with: --build${NC}"
    exit 1
fi

if [ ! -f "$MEROCTL_BIN" ]; then
    echo -e "${RED}Error: meroctl binary not found at $MEROCTL_BIN${NC}"
    echo -e "${YELLOW}Build it with: cargo build -p meroctl${NC}"
    echo -e "${YELLOW}Or run this script with: --build${NC}"
    exit 1
fi

echo -e "${GREEN}✓ Found merod:${NC} $MEROD_BIN"
echo -e "${GREEN}✓ Found meroctl:${NC} $MEROCTL_BIN"

# Check if apps are built
KV_STORE_WASM="${PROJECT_ROOT}/apps/kv-store/res/kv_store.wasm"
if [ ! -f "$KV_STORE_WASM" ]; then
    echo -e "${RED}Error: kv_store.wasm not found${NC}"
    echo -e "${YELLOW}Build it with: ./apps/kv-store/build.sh${NC}"
    echo -e "${YELLOW}Or run this script with: --build-apps${NC}"
    exit 1
fi

echo -e "${GREEN}✓ Found kv_store.wasm${NC}"
echo ""

# Run tests
run_test() {
    local workflow_file=$1
    local protocol_name=$2
    
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BLUE}Running: $protocol_name${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    
    local output_dir="${RESULTS_DIR}/${protocol_name}"
    mkdir -p "$output_dir"
    
    # Run merobox workflow
    # Command: merobox bootstrap run [config_file] --no-docker
    if merobox bootstrap run \
        "$workflow_file" \
        --no-docker \
        $VERBOSE; then
        echo ""
        echo -e "${GREEN}✓ $protocol_name: PASSED${NC}"
        echo ""
        return 0
    else
        echo ""
        echo -e "${RED}✗ $protocol_name: FAILED${NC}"
        echo ""
        return 1
    fi
}

# Main execution
FAILED=0

if [ -n "$WORKFLOW" ]; then
    # Run custom workflow
    if [ ! -f "$WORKFLOW" ]; then
        echo -e "${RED}Error: Workflow file not found: $WORKFLOW${NC}"
        exit 1
    fi
    run_test "$WORKFLOW" "custom"
    FAILED=$?
else
    # Run protocol-specific or all tests
    case $PROTOCOL in
        near)
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store/near.yml" "near"
            FAILED=$?
            ;;
        icp)
            echo -e "${YELLOW}Note: ICP tests require dfx and a running ICP devnet${NC}"
            echo -e "${YELLOW}Run: ./scripts/icp/deploy-devnet.sh${NC}"
            echo ""
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store/icp.yml" "icp"
            FAILED=$?
            ;;
        ethereum)
            echo -e "${YELLOW}Note: Ethereum tests require Foundry and a running devnet${NC}"
            echo -e "${YELLOW}Run: ./scripts/ethereum/deploy-devnet.sh${NC}"
            echo ""
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store/ethereum.yml" "ethereum"
            FAILED=$?
            ;;
        near-proposals)
            echo -e "${YELLOW}Running NEAR proposals comprehensive test...${NC}"
            echo ""
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/proposals/near-proposals.yml" "near-proposals"
            FAILED=$?
            ;;
        all)
            echo -e "${YELLOW}Running all protocols...${NC}"
            echo ""
            
            # Run NEAR (doesn't need devnet)
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store/near.yml" "near"
            NEAR_RESULT=$?
            
            # Check if ICP devnet is available
            if pgrep -f "dfx" > /dev/null; then
                run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store/icp.yml" "icp"
                ICP_RESULT=$?
            else
                echo -e "${YELLOW}⊘ ICP: SKIPPED (dfx not running)${NC}"
                ICP_RESULT=0
            fi
            
            # Check if Ethereum devnet is available
            if pgrep -f "anvil" > /dev/null; then
                run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store/ethereum.yml" "ethereum"
                ETH_RESULT=$?
            else
                echo -e "${YELLOW}⊘ Ethereum: SKIPPED (anvil not running)${NC}"
                ETH_RESULT=0
            fi
            
            FAILED=$((NEAR_RESULT + ICP_RESULT + ETH_RESULT))
            ;;
        "")
            echo -e "${RED}Error: Protocol not specified${NC}"
            usage
            exit 1
            ;;
        *)
            echo -e "${RED}Error: Unknown protocol: $PROTOCOL${NC}"
            usage
            exit 1
            ;;
    esac
fi

# Summary
echo ""
echo -e "${BLUE}╔═══════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║  Test Summary                                            ║${NC}"
echo -e "${BLUE}╚═══════════════════════════════════════════════════════════╝${NC}"
echo ""

# Cleanup: Deactivate virtual environment if we used it
if [ "$USE_VENV" = true ] && [ -n "$VIRTUAL_ENV" ]; then
    echo -e "${BLUE}Cleaning up virtual environment...${NC}"
    deactivate 2>/dev/null || true
fi

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}All tests passed!${NC}"
    echo ""
    echo -e "Results saved to: ${RESULTS_DIR}"
    echo ""
    exit 0
else
    echo -e "${RED}Some tests failed (failed: $FAILED)${NC}"
    echo ""
    echo -e "Check results in: ${RESULTS_DIR}"
    echo -e "Check logs in: ~/.merobox/logs/"
    echo ""
    exit 1
fi

