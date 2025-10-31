# Merobox E2E Tests

These tests use [Merobox](https://github.com/calimero-network/merobox) for e2e test of Calimere core.

## 📁 Directory Structure

```
e2e-tests-merobox/
├── workflows/
│   ├── kv-store/           # KV Store test workflows
│   │   ├── near.yml        # NEAR protocol (basic KV store)
│   │   ├── near-init.yml   # NEAR protocol (KV store with init())
│   │   ├── icp.yml         # ICP protocol
│   │   └── ethereum.yml    # Ethereum protocol
│   ├── kv-store-with-handlers/ # KV Store with Handlers workflows
│   │   └── near.yml        # NEAR protocol (handlers test)
│   ├── blobs/              # Blob API workflows
│   │   └── near.yml        # NEAR protocol (blob API test)
│   ├── collaborative-editor/ # Collaborative Editor workflows
│   │   ├── near.yml        # NEAR protocol (CRDT text editing)
│   ├── nested-crdt/        # Nested CRDT workflows
│   │   ├── near.yml        # NEAR protocol (nested CRDT compositions)
│   ├── team-metrics/       # Team Metrics workflows
│   │   └── near.yml        # NEAR protocol (#[derive(Mergeable)] testing)
│   ├── concurrent-mutations/ # Concurrent Mutations workflows
│   │   └── near.yml        # NEAR protocol (DAG convergence testing)
│   ├── open-invitation/    # Open Invitation workflows
│   │   ├── near.yml        # NEAR protocol (open invitation join)
│   │   └── README.md       # Open invitation testing guide
│   ├── xcall-example/      # Cross-context XCall workflows
│   │   ├── near.yml        # NEAR protocol (ping-pong between contexts)
│   └── proposals/          # Proposals test workflows
│       ├── near-proposals.yml  # NEAR proposals comprehensive test
│       ├── icp-proposals.yml   # ICP proposals comprehensive test
│       ├── ethereum-proposals.yml # Ethereum proposals comprehensive test
│       └── README.md       # Proposals testing guide
├── results/                # Test output (generated)
├── run-local.sh           # Local test runner script
└── README.md              # This file
```

## 🚀 Quick Start

### Prerequisites

1. **Python 3.11+** (for Merobox)
2. **Rust toolchain** (for building binaries)
3. **Git** (for cloning merobox source)
4. **Merobox** - No manual installation needed!
   - The test script automatically creates a virtual environment
   - Clones merobox from source (https://github.com/calimero-network/merobox)
   - Installs merobox in editable mode (`pip install -e`)
   - This avoids Python GIL errors and environment conflicts
   - Currently tested with merobox >= 0.2.0
   - Use `--no-venv` flag if you want to use a manually installed merobox (not recommended)

### Build Binaries

```bash
# Build Calimero binaries
cargo build -p merod -p meroctl

# Build test applications
./apps/kv-store/build.sh
./apps/kv-store-init/build.sh
./apps/kv-store-with-handlers/build.sh
./apps/blobs/build.sh
./apps/collaborative-editor/build.sh
./apps/nested-crdt-test/build.sh
./apps/team-metrics-macro/build.sh
# Note: concurrent-mutations uses kv-store app (already built above)
```

### Run Tests Locally

```bash
# Run NEAR KV Store test (no devnet needed)
./e2e-tests-merobox/run-local.sh --protocol near --build

# Run NEAR KV Store Init test (tests init() and len() methods)
./e2e-tests-merobox/run-local.sh --protocol near-init --build --build-apps

# Run NEAR proposals comprehensive test
./e2e-tests-merobox/run-local.sh --protocol near-proposals --build --build-apps

# Run ICP proposals comprehensive test
./e2e-tests-merobox/run-local.sh --protocol icp-proposals --build --build-apps --check-devnets

# Run Ethereum proposals comprehensive test
./e2e-tests-merobox/run-local.sh --protocol ethereum-proposals --build --build-apps --check-devnets

# Run ICP tests (with devnet check)
./e2e-tests-merobox/run-local.sh --protocol icp --build --check-devnets

# Run Ethereum tests (with devnet check)
./e2e-tests-merobox/run-local.sh --protocol ethereum --build --check-devnets

# Run Open Invitation tests
./e2e-tests-merobox/run-local.sh --protocol near-open-invitation --build --build-apps

# Run XCall example tests
./e2e-tests-merobox/run-local.sh --protocol near-xcall --build --build-apps

# Run all protocols (KV Store + Init + Handlers + Blobs + Collaborative Editor + Nested CRDT + Team Metrics + Concurrent Mutations + Open Invitation + XCall + Proposals)
./e2e-tests-merobox/run-local.sh --protocol all --build --build-apps

# Note: This runs 15 test suites:
# - KV Store: near, near-init, icp (if dfx running), ethereum (if anvil running)
# - Handlers: near-handlers
# - Blob API: near-blobs
# - Collaborative Editor: near-collab
# - Nested CRDT: near-nested
# - Team Metrics: near-metrics
# - Concurrent Mutations: near-concurrent
# - Open Invitation: near-open-invitation
# - XCall Example: near-xcall
# - Proposals: near-proposals, icp-proposals (if dfx), ethereum-proposals (if anvil)

# Run KV Store with Handlers test (NEAR only)
./e2e-tests-merobox/run-local.sh --protocol near-handlers --build --build-apps

# Run Blob API test (NEAR only)
./e2e-tests-merobox/run-local.sh --protocol near-blobs --build --build-apps

# Run Collaborative Editor test (NEAR only)
./e2e-tests-merobox/run-local.sh --protocol near-collab --build --build-apps

# Run Nested CRDT test (NEAR only)
./e2e-tests-merobox/run-local.sh --protocol near-nested --build --build-apps

# Run Team Metrics test (NEAR only)
./e2e-tests-merobox/run-local.sh --protocol near-metrics --build --build-apps

# Run Concurrent Mutations test (NEAR only)
./e2e-tests-merobox/run-local.sh --protocol near-concurrent --build --build-apps

# Run custom workflow
./e2e-tests-merobox/run-local.sh --workflow path/to/custom.yml
```

**What Happens Automatically**:

1. Creates fresh virtual environment at `.venv-merobox/`
2. Installs merobox in the virtual environment
3. Checks devnet status (if `--check-devnets` used)
4. Runs tests with isolated Python environment
5. Cleans up after completion

**Available Flags**:

- `-p, --protocol`: Protocol to test (near, icp, ethereum, all, or near-proposals)
- `-w, --workflow`: Path to custom workflow YAML file
- `-b, --build`: Build merod and meroctl binaries before testing
- `-a, --build-apps`: Build WASM applications before testing
- `-c, --check-devnets`: Check if devnets are running (shows setup instructions if not)
- `-v, --verbose`: Enable verbose output
- `--no-venv`: Don't use virtual environment (not recommended)

### Setup Devnets (Required for ICP/Ethereum)

Before running ICP or Ethereum tests, deploy the respective devnets:

```bash
# Deploy ICP devnet (requires dfx)
./scripts/icp/deploy-devnet.sh

# Deploy Ethereum devnet (requires Foundry)
./scripts/ethereum/deploy-devnet.sh
```

## 📝 Writing Tests

### Workflow Structure

Each workflow is a YAML file with the following structure:

```yaml
name: "Test Name"
description: "Test description"

# Use native merod processes (--no-docker)
use_docker: false

nodes:
  count: 3
  prefix: test-prefix
  chain_id: calimero-testnet

steps:
  - name: Step name
    type: step_type
    node: node-name
    # ... step-specific parameters
```

### Available Step Types

| Step Type              | Description              | Example                |
| ---------------------- | ------------------------ | ---------------------- |
| `install_application`  | Install WASM app         | Install kv_store.wasm  |
| `create_context`       | Create execution context | Create new context     |
| `create_context_alias` | Add alias to context     | Alias as "my_context"  |
| `create_identity`      | Generate node identity   | For invite flow        |
| `invite_identity`      | Invite node to context   | Invite node 2          |
| `join_context`         | Join context             | Node 2 joins           |
| `call`                 | Execute contract method  | `set`, `get`, `remove` |
| `wait`                 | Delay execution          | Wait for consensus     |

### Variable Substitution

Use `{{variable_name}}` to reference outputs from previous steps:

```yaml
- name: Create Context
  type: create_context
  outputs:
    context_id: contextId
    inviter_key: memberPublicKey

- name: Use Context
  type: call
  context_id: "{{context_id}}"
  executor_public_key: "{{inviter_key}}"
```

### Example: Simple KV Store Test

```yaml
name: "Simple KV Test"
use_docker: false
nodes:
  count: 2
  prefix: simple-kv
  chain_id: calimero-testnet

steps:
  - name: Install App
    type: install_application
    node: simple-kv-1
    path: "apps/kv-store/res/kv_store.wasm"
    outputs:
      app_id: applicationId

  - name: Create Context
    type: create_context
    node: simple-kv-1
    application_id: "{{app_id}}"
    outputs:
      context_id: contextId
      inviter_key: memberPublicKey

  - name: Set Value
    type: call
    node: simple-kv-1
    context_id: "{{context_id}}"
    executor_public_key: "{{inviter_key}}"
    method: set
    args:
      key: "test"
      value: "hello"

  - name: Get Value
    type: call
    node: simple-kv-1
    context_id: "{{context_id}}"
    executor_public_key: "{{inviter_key}}"
    method: get
    args:
      key: "test"
    expected_output: "hello"
```

## 🔧 Local Development

### Running Individual Tests

```bash
# NEAR (no external dependencies needed)
./e2e-tests-merobox/run-local.sh --protocol near --verbose

# ICP (requires dfx)
./scripts/icp/deploy-devnet.sh
./e2e-tests-merobox/run-local.sh --protocol icp

# Ethereum (requires foundry)
./scripts/ethereum/deploy-devnet.sh
./e2e-tests-merobox/run-local.sh --protocol ethereum
```

### Debugging

Enable verbose output:

```bash
./e2e-tests-merobox/run-local.sh --protocol near --verbose
```

Check node logs:

```bash
# Merobox stores logs in ~/.merobox/logs/
tail -f ~/.merobox/logs/calimero-node-1.log
```

View test results:

```bash
# Results are saved in e2e-tests-merobox/results/
cat e2e-tests-merobox/results/near/summary.json
```

### Manual Workflow Execution

You can also run merobox directly:

```bash
# Merobox 0.2.0+ command structure
merobox bootstrap run \
  e2e-tests-merobox/workflows/kv-store/near.yml \
  --no-docker \
  --verbose
```

## 📝 Test Results & Logs

After running tests locally, results are automatically saved to:

```
e2e-tests-merobox/results/{protocol}/
├── test.log        # Full test output (stdout + stderr)
└── summary.json    # Test summary with duration, steps, status
```

### Example Summary

```json
{
  "status": "passed",
  "protocol": "near",
  "duration": 45,
  "total_steps": 48,
  "passed_steps": 48,
  "failed_steps": 0,
  "workflow": "e2e-tests-merobox/workflows/kv-store/near.yml",
  "timestamp": "2025-10-26T12:30:00Z"
}
```

### Viewing Results

```bash
# View test log
cat e2e-tests-merobox/results/near/test.log

# View summary (with jq for pretty formatting)
cat e2e-tests-merobox/results/near/summary.json | jq

# List all test results
ls -la e2e-tests-merobox/results/

# View recent logs
tail -f e2e-tests-merobox/results/near/test.log
```

## 🤖 CI/CD Integration

Tests run automatically on GitHub Actions via `.github/workflows/e2e-tests-merobox.yml`.

### Workflow Triggers

- Push to `master` or `feature/merobox-e2e-migration`
- Pull requests affecting relevant code

### Matrix Strategy

Tests run in parallel for each protocol:

- NEAR (no external dependencies)
- ICP (with dfx devnet)
- Ethereum (with anvil devnet)

### Artifacts

The CI workflow uploads:

- Test results (`merobox-kv-store-{protocol}`)
- Node logs (`merobox-logs-{protocol}`)
- Summary report (PR comment)

## 📊 Test Coverage

### Current Tests

| Test Suite                 | Protocols           | Steps | Status      |
| -------------------------- | ------------------- | ----- | ----------- |
| **KV Store**               | NEAR, ICP, Ethereum | ~48   | Implemented |
| **KV Store Init**          | NEAR                | ~50   | Implemented |
| **KV Store with Handlers** | NEAR                | ~35   | Implemented |
| **Blob API**               | NEAR                | ~30   | Implemented |
| **Collaborative Editor**   | NEAR                | ~50   | Implemented |
| **Nested CRDT**            | NEAR                | ~80   | Implemented |
| **Team Metrics**           | NEAR                | ~45   | Implemented |
| **Concurrent Mutations**   | NEAR                | ~35   | Implemented |
| **Proposals**              | NEAR, ICP, Ethereum | 70+   | Implemented |

### Planned Tests

- KV Store Init (ICP, Ethereum)
- KV Store with Handlers (ICP, Ethereum)
- Blob API (ICP, Ethereum)
- Collaborative Editor (ICP, Ethereum)
- Nested CRDT (ICP, Ethereum)
- Team Metrics (ICP, Ethereum)
- Concurrent Mutations (ICP, Ethereum)
- Open Invitations (NEAR, ICP, Ethereum) - requires merobox support
- External State Verification (all protocols)

## 🔄 Migration Status

This is a **parallel implementation** of the existing Rust-based e2e tests. Both test suites will run simultaneously during the migration period.

### Migration Phases

- [x] **Phase 1**: KV Store tests for NEAR, ICP, Ethereum
- [x] **Phase 2**: Proposals API comprehensive testing
- [ ] **Phase 3**: Advanced features (handlers, open invitations)
- [ ] **Phase 4**: Complete feature parity + new tests
- [ ] **Phase 5**: Deprecate Rust tests

## 🐛 Troubleshooting

### Common Issues

**1. Merobox not found or crashes**

If merobox is not installed:

```bash
pip install merobox
```

If merobox crashes with Python GIL error:

```bash
# This is often due to Python environment issues on macOS
# Solution 1: Use a virtual environment (recommended)
python3 -m venv venv
source venv/bin/activate
pip install --upgrade pip
pip install merobox

# Solution 2: Reinstall with --force
pip uninstall merobox -y
pip install --no-cache-dir merobox

# Solution 3: Use system Python (if in a problematic venv)
deactivate  # if in venv
python3 -m pip install --user merobox

# Verify installation
merobox --version
```

**2. Binaries not found**

```bash
# Option 1: Use the build flag
./e2e-tests-merobox/run-local.sh --protocol near --build

# Option 2: Build manually
cargo build -p merod -p meroctl
```

**3. WASM not found**

```bash
# Option 1: Use the build-apps flag
./e2e-tests-merobox/run-local.sh --protocol near --build-apps

# Option 2: Build manually
./apps/kv-store/build.sh
```

**4. Port conflicts**

```bash
# Stop any running merod processes
pkill -f merod

# Clean up merobox state
rm -rf ~/.merobox/nodes/
```

**5. ICP tests fail**

```bash
# Make sure dfx is running
pgrep dfx

# Restart devnet
./scripts/icp/nuke-devnet.sh
./scripts/icp/deploy-devnet.sh
```

**6. Ethereum tests fail**

```bash
# Make sure anvil is running
pgrep anvil

# Restart devnet
./scripts/ethereum/nuke-devnet.sh
./scripts/ethereum/deploy-devnet.sh
```
