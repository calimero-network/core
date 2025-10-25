# Merobox E2E Tests

This directory contains the next-generation E2E tests for Calimero, powered by **Merobox** with `--no-docker` mode.

## 🎯 Overview

These tests use [Merobox](https://github.com/calimero-network/merobox), a Python-based workflow orchestrator that manages Calimero nodes natively (without Docker containers), making tests faster and simpler to run both locally and in CI.

### Key Benefits

- ✅ **Fast**: Native processes, no Docker overhead
- ✅ **Simple**: YAML-based workflow definitions
- ✅ **Maintainable**: Declarative syntax, easy to understand
- ✅ **CI-friendly**: Quick startup, clean shutdown
- ✅ **Local-first**: Same experience locally and in CI

## 📁 Directory Structure

```
e2e-tests-merobox/
├── workflows/
│   ├── kv-store/           # KV Store test workflows
│   │   ├── near.yml        # NEAR protocol
│   │   ├── icp.yml         # ICP protocol
│   │   └── ethereum.yml    # Ethereum protocol
│   └── proposals/          # Proposals test workflows
│       ├── near-proposals.yml  # NEAR proposals comprehensive test
│       └── README.md       # Proposals testing guide
├── results/                # Test output (generated)
├── run-local.sh           # Local test runner script
└── README.md              # This file
```

## 🚀 Quick Start

### Prerequisites

1. **Python 3.11+** (for Merobox)
2. **Rust toolchain** (for building binaries)
3. **Merobox** - No manual installation needed!
   - The test script automatically creates a virtual environment and installs merobox
   - This avoids Python GIL errors and environment conflicts
   - Currently tested with merobox >= 0.2.0
   - Use `--no-venv` flag if you want to use a manually installed merobox (not recommended)

### Build Binaries

```bash
# Build Calimero binaries
cargo build -p merod -p meroctl

# Build test applications
./apps/kv-store/build.sh
```

### Run Tests Locally

```bash
# Run NEAR KV store tests (automatically sets up venv + merobox)
./e2e-tests-merobox/run-local.sh --protocol near

# Run NEAR proposals comprehensive test
./e2e-tests-merobox/run-local.sh --protocol near-proposals --build --build-apps

# Run with auto-build (builds binaries + apps automatically)
./e2e-tests-merobox/run-local.sh --protocol near --build --build-apps

# Build everything and run all KV store protocols
./e2e-tests-merobox/run-local.sh --protocol all --build --build-apps --verbose

# Run custom workflow
./e2e-tests-merobox/run-local.sh --workflow path/to/custom.yml
```

**What Happens Automatically**:

1. 🔄 Creates fresh virtual environment at `.venv-merobox/`
2. 📦 Installs merobox in the virtual environment
3. ✅ Runs tests with isolated Python environment
4. 🧹 Cleans up after completion

**Available Flags**:

- `-p, --protocol`: Protocol to test (near, icp, ethereum, or all)
- `-w, --workflow`: Path to custom workflow YAML file
- `-b, --build`: Build merod and meroctl binaries before testing
- `-a, --build-apps`: Build WASM applications before testing
- `-v, --verbose`: Enable verbose output
- `--no-venv`: Don't use virtual environment (not recommended)

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

| Test Suite    | Protocols           | Steps | Status         |
| ------------- | ------------------- | ----- | -------------- |
| **KV Store**  | NEAR, ICP, Ethereum | ~48   | ✅ Implemented |
| **Proposals** | NEAR                | 70+   | ✅ Implemented |

### Planned Tests

- KV Store with Handlers (NEAR)
- Open Invitations (NEAR)
- Proposals API (ICP, Ethereum)
- External State Verification (all protocols)

## 🔄 Migration Status

This is a **parallel implementation** of the existing Rust-based e2e tests. Both test suites will run simultaneously during the migration period.

### Migration Phases

1. **Phase 1** (Current): KV Store tests for NEAR, ICP, Ethereum
2. **Phase 2**: Proposals API comprehensive testing
3. **Phase 3**: Advanced features (handlers, open invitations)
4. **Phase 4**: Complete feature parity + new tests
5. **Phase 5**: Deprecate Rust tests

See `MEROBOX_MIGRATION_PLAN.md` and related docs in the project root for details.

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

## 📚 Additional Resources

- [Merobox Documentation](https://github.com/calimero-network/merobox)
- [Migration Plan](../MEROBOX_MIGRATION_PLAN.md)
- [Migration Index](../MEROBOX_MIGRATION_INDEX.md)
- [No-Docker Mode Guide](../MEROBOX_MIGRATION_NO_DOCKER.md)
- [Proposals Testing Guide](../MEROBOX_MIGRATION_PROPOSALS_GUIDE.md)
- [Current E2E Tests Inventory](../CURRENT_E2E_TESTS_INVENTORY.md)

## 🤝 Contributing

When adding new tests:

1. Create workflow YAML in `workflows/`
2. Follow existing naming conventions
3. Add comprehensive comments
4. Test locally before pushing
5. Update this README with new test coverage

### Workflow Best Practices

- ✅ Use descriptive step names
- ✅ Group related steps with comments
- ✅ Add `outputs:` to capture important values
- ✅ Use `expected_output` for verification
- ✅ Add appropriate `wait` steps for consensus
- ✅ Test with `--verbose` locally first

## 📞 Support

For issues, questions, or contributions:

- Open an issue on GitHub
- Check existing migration documentation
- Review merobox documentation

---

**Status**: 🚧 Active Development  
**Last Updated**: October 2025  
**Maintainers**: Calimero Core Team
