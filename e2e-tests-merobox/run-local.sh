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
CHECK_DEVNETS=false
USE_VENV=true
VENV_DIR="${PROJECT_ROOT}/.venv-merobox"

# Print usage
usage() {
    echo -e "${BLUE}Usage:${NC} $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  -p, --protocol PROTOCOL    Protocol to test:"
    echo "                             - near, icp, ethereum (KV Store only)"
    echo "                             - near-handlers (KV Store with Handlers - NEAR only)"
    echo "                             - near-blobs (Blob API - NEAR only)"
    echo "                             - near-proposals, icp-proposals, ethereum-proposals"
    echo "                             - all (runs all tests: KV Store + Handlers + Blobs + Proposals)"
    echo "  -w, --workflow WORKFLOW    Path to workflow YAML file (overrides protocol)"
    echo "  -v, --verbose              Enable verbose output"
    echo "  -b, --build                Build merod and meroctl binaries before testing"
    echo "  -a, --build-apps           Build WASM applications before testing"
    echo "  -c, --check-devnets        Check if devnets are running (shows setup instructions if not)"
    echo "  --no-venv                  Don't use virtual environment (not recommended)"
    echo "  -h, --help                 Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 --protocol near --build                     # Run NEAR KV Store tests"
    echo "  $0 --protocol near-handlers --build --build-apps  # Run NEAR Handlers tests"
    echo "  $0 --protocol near-blobs --build --build-apps  # Run NEAR Blob API tests"
    echo "  $0 --protocol icp --check-devnets --build      # Check ICP devnet and test"
    echo "  $0 --protocol all --build --build-apps         # Build and test all (8 suites)"
    echo "  $0 --workflow path/to/custom.yml               # Run custom workflow"
    echo ""
    echo "Devnet Setup (run separately before testing):"
    echo "  ./scripts/icp/deploy-devnet.sh                 # Deploy ICP devnet"
    echo "  ./scripts/ethereum/deploy-devnet.sh            # Deploy Ethereum devnet"
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
        -c|--check-devnets)
            CHECK_DEVNETS=true
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
    if python3.11 -m venv "$VENV_DIR"; then
        echo -e "${GREEN}✓ Virtual environment created${NC}"
    else
        echo -e "${RED}Error: Failed to create virtual environment${NC}"
        echo -e "${YELLOW}Make sure python3.11-venv is installed${NC}"
        exit 1
    fi
    
    # Activate virtual environment
    source "$VENV_DIR/bin/activate"
    
    # Upgrade pip
    echo -e "${BLUE}Upgrading pip...${NC}"
    pip install --upgrade pip > /dev/null 2>&1
    
    # Clone or update merobox from source
    MEROBOX_DIR="${PROJECT_ROOT}/.merobox-src"
    if [ -d "$MEROBOX_DIR" ]; then
        echo -e "${BLUE}Updating merobox from source...${NC}"
        cd "$MEROBOX_DIR"
        git pull origin master > /dev/null 2>&1 || true
        cd "$PROJECT_ROOT"
    else
        echo -e "${BLUE}Cloning merobox from source...${NC}"
        if git clone https://github.com/calimero-network/merobox.git "$MEROBOX_DIR"; then
            echo -e "${GREEN}✓ Merobox cloned successfully${NC}"
        else
            echo -e "${RED}Error: Failed to clone merobox${NC}"
            exit 1
        fi
    fi
    
    # Install merobox from source (editable mode)
    echo -e "${BLUE}Installing merobox from source (editable mode)...${NC}"
    if pip install -e "$MEROBOX_DIR"; then
        echo -e "${GREEN}✓ Merobox installed successfully from source${NC}"
    else
        echo -e "${RED}Error: Failed to install merobox from source${NC}"
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
    
    # Make build scripts executable
    chmod +x ./apps/kv-store/build.sh
    chmod +x ./apps/kv-store-with-handlers/build.sh
    chmod +x ./apps/blobs/build.sh
    
    if ./apps/kv-store/build.sh; then
        echo -e "${GREEN}✓ KV store app built successfully${NC}"
    else
        echo -e "${RED}Error: Failed to build kv-store app${NC}"
        exit 1
    fi
    
    if ./apps/kv-store-with-handlers/build.sh; then
        echo -e "${GREEN}✓ KV store with handlers app built successfully${NC}"
    else
        echo -e "${RED}Error: Failed to build kv-store-with-handlers app${NC}"
        exit 1
    fi
    
    if ./apps/blobs/build.sh; then
        echo -e "${GREEN}✓ Blobs app built successfully${NC}"
    else
        echo -e "${RED}Error: Failed to build blobs app${NC}"
        exit 1
    fi
    echo ""
fi

# Check devnet status if requested
if [ "$CHECK_DEVNETS" = true ]; then
    echo -e "${BLUE}Checking devnet status...${NC}"
    echo ""
    
    # Check ICP devnet
    if [[ "$PROTOCOL" == "icp" || "$PROTOCOL" == "all" ]]; then
        echo -e "${BLUE}Checking ICP devnet...${NC}"
        
        if pgrep -f "dfx" > /dev/null; then
            echo -e "${GREEN}✓ ICP devnet is running (dfx process found)${NC}"
        else
            echo -e "${YELLOW}✗ ICP devnet is NOT running${NC}"
            echo -e "${BLUE}To deploy ICP devnet:${NC}"
            echo -e "  1. Install dfx: ${YELLOW}sh -ci \"\$(curl -fsSL https://internetcomputer.org/install.sh)\"${NC}"
            echo -e "  2. Deploy devnet: ${YELLOW}./scripts/icp/deploy-devnet.sh${NC}"
            echo ""
            
            if [[ "$PROTOCOL" == "icp" ]]; then
                echo -e "${RED}Error: ICP devnet required for ICP tests${NC}"
                exit 1
            fi
        fi
        echo ""
    fi
    
    # Check Ethereum devnet
    if [[ "$PROTOCOL" == "ethereum" || "$PROTOCOL" == "all" ]]; then
        echo -e "${BLUE}Checking Ethereum devnet...${NC}"
        
        if pgrep -f "anvil" > /dev/null; then
            echo -e "${GREEN}✓ Ethereum devnet is running (anvil process found)${NC}"
        else
            echo -e "${YELLOW}✗ Ethereum devnet is NOT running${NC}"
            echo -e "${BLUE}To deploy Ethereum devnet:${NC}"
            echo -e "  1. Install Foundry: ${YELLOW}curl -L https://foundry.paradigm.xyz | bash${NC}"
            echo -e "  2. Setup Foundry: ${YELLOW}foundryup${NC}"
            echo -e "  3. Deploy devnet: ${YELLOW}./scripts/ethereum/deploy-devnet.sh${NC}"
            echo ""
            
            if [[ "$PROTOCOL" == "ethereum" ]]; then
                echo -e "${RED}Error: Ethereum devnet required for Ethereum tests${NC}"
                exit 1
            fi
        fi
        echo ""
    fi
    
    echo -e "${GREEN}✓ Devnet status check complete${NC}"
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
    
    local log_file="${output_dir}/test.log"
    local summary_file="${output_dir}/summary.json"
    local start_time=$(date +%s)
    
    echo -e "${BLUE}Logs will be saved to: ${log_file}${NC}"
    echo ""
    
    # Run merobox workflow and capture output
    # Command: merobox bootstrap run [config_file] --no-docker --binary-path /path/to/merod
    # Use pipefail to capture exit code even when piped through tee
    set -o pipefail
    merobox bootstrap run \
        "$workflow_file" \
        --no-docker \
        --binary-path "$MEROD_BIN" \
        $VERBOSE 2>&1 | tee "$log_file"
    local exit_code=${PIPESTATUS[0]}
    set +o pipefail
    
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    # Extract step counts from log
    local total_steps=$(grep -c "Step " "$log_file" || echo "0")
    local passed_steps=$(grep -c "✓\|✅\|succeeded\|completed" "$log_file" || echo "0")
    local failed_steps=$(grep -c "✗\|❌\|failed\|error" "$log_file" || echo "0")
    
    # Check for failure indicators in the log
    local has_failure=false
    if grep -q "Workflow failed\|Step.*failed\|ERROR\|Error:" "$log_file"; then
        has_failure=true
    fi
    
    # Determine if test passed (exit code 0 AND no failure indicators)
    if [ "$exit_code" -eq 0 ] && [ "$has_failure" = false ]; then
        # Test PASSED
        cat > "$summary_file" <<EOF
{
  "status": "passed",
  "protocol": "${protocol_name}",
  "duration": ${duration},
  "total_steps": ${total_steps},
  "passed_steps": ${passed_steps},
  "failed_steps": ${failed_steps},
  "workflow": "${workflow_file}",
  "timestamp": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
}
EOF
        
        echo ""
        echo -e "${GREEN}✓ $protocol_name: PASSED${NC}"
        echo -e "${BLUE}Duration: ${duration}s${NC}"
        echo -e "${BLUE}Results saved to: ${output_dir}${NC}"
        echo ""
        return 0
    else
        # Test FAILED
        cat > "$summary_file" <<EOF
{
  "status": "failed",
  "protocol": "${protocol_name}",
  "duration": ${duration},
  "exit_code": ${exit_code},
  "total_steps": ${total_steps},
  "passed_steps": ${passed_steps},
  "failed_steps": ${failed_steps},
  "workflow": "${workflow_file}",
  "timestamp": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
}
EOF
        
        echo ""
        echo -e "${RED}✗ $protocol_name: FAILED${NC}"
        echo -e "${BLUE}Duration: ${duration}s${NC}"
        echo -e "${BLUE}Exit code: ${exit_code}${NC}"
        echo -e "${BLUE}Logs saved to: ${log_file}${NC}"
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
        near-handlers)
            echo -e "${YELLOW}Running NEAR KV Store with Handlers test...${NC}"
            echo ""
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store-with-handlers/near.yml" "near-handlers"
            FAILED=$?
            ;;
        near-blobs)
            echo -e "${YELLOW}Running NEAR Blob API test...${NC}"
            echo ""
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/blobs/near.yml" "near-blobs"
            FAILED=$?
            ;;
        near-proposals)
            echo -e "${YELLOW}Running NEAR proposals comprehensive test...${NC}"
            echo ""
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/proposals/near-proposals.yml" "near-proposals"
            FAILED=$?
            ;;
        icp-proposals)
            echo -e "${YELLOW}Running ICP proposals comprehensive test...${NC}"
            echo -e "${YELLOW}Note: Requires ICP devnet (dfx) to be running${NC}"
            echo ""
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/proposals/icp-proposals.yml" "icp-proposals"
            FAILED=$?
            ;;
        ethereum-proposals)
            echo -e "${YELLOW}Running Ethereum proposals comprehensive test...${NC}"
            echo -e "${YELLOW}Note: Requires Ethereum devnet (anvil) to be running${NC}"
            echo ""
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/proposals/ethereum-proposals.yml" "ethereum-proposals"
            FAILED=$?
            ;;
        all)
            echo -e "${YELLOW}Running all protocols (KV Store + Proposals)...${NC}"
            echo ""
            
            # === KV Store Tests ===
            echo -e "${BLUE}━━━ KV Store Tests ━━━${NC}"
            echo ""
            
            # Run NEAR KV Store 
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store/near.yml" "near"
            NEAR_KV_RESULT=$?
            
            # Check if ICP devnet is available
            if pgrep -f "dfx" > /dev/null; then
                run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store/icp.yml" "icp"
                ICP_KV_RESULT=$?
            else
                echo -e "${YELLOW}⊘ ICP KV Store: SKIPPED (dfx not running)${NC}"
                ICP_KV_RESULT=0
            fi
            
            # Check if Ethereum devnet is available
            if pgrep -f "anvil" > /dev/null; then
                run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store/ethereum.yml" "ethereum"
                ETH_KV_RESULT=$?
            else
                echo -e "${YELLOW}⊘ Ethereum KV Store: SKIPPED (anvil not running)${NC}"
                ETH_KV_RESULT=0
            fi
            
            # === KV Store with Handlers Tests ===
            echo ""
            echo -e "${BLUE}━━━ KV Store with Handlers Tests ━━━${NC}"
            echo ""
            
            # Run NEAR Handlers (doesn't need devnet)
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/kv-store-with-handlers/near.yml" "near-handlers"
            NEAR_HANDLERS_RESULT=$?
            
            # === Blob API Tests ===
            echo ""
            echo -e "${BLUE}━━━ Blob API Tests ━━━${NC}"
            echo ""
            
            # Run NEAR Blobs (doesn't need devnet)
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/blobs/near.yml" "near-blobs"
            NEAR_BLOBS_RESULT=$?
            
            # === Proposals Tests ===
            echo ""
            echo -e "${BLUE}━━━ Proposals Tests ━━━${NC}"
            echo ""
            
            # Run NEAR Proposals (doesn't need devnet)
            run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/proposals/near-proposals.yml" "near-proposals"
            NEAR_PROP_RESULT=$?
            
            # Run ICP Proposals if devnet available
            if pgrep -f "dfx" > /dev/null; then
                run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/proposals/icp-proposals.yml" "icp-proposals"
                ICP_PROP_RESULT=$?
            else
                echo -e "${YELLOW}⊘ ICP Proposals: SKIPPED (dfx not running)${NC}"
                ICP_PROP_RESULT=0
            fi
            
            # Run Ethereum Proposals if devnet available
            if pgrep -f "anvil" > /dev/null; then
                run_test "${PROJECT_ROOT}/e2e-tests-merobox/workflows/proposals/ethereum-proposals.yml" "ethereum-proposals"
                ETH_PROP_RESULT=$?
            else
                echo -e "${YELLOW}⊘ Ethereum Proposals: SKIPPED (anvil not running)${NC}"
                ETH_PROP_RESULT=0
            fi
            
            FAILED=$((NEAR_KV_RESULT + ICP_KV_RESULT + ETH_KV_RESULT + NEAR_HANDLERS_RESULT + NEAR_BLOBS_RESULT + NEAR_PROP_RESULT + ICP_PROP_RESULT + ETH_PROP_RESULT))
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

