# E2e tests

Binary crate which runs e2e tests for the merod node.

## Usage

First build apps, contracts, and mero binaries. After that deploy the devnet (if
needed) and run the e2e tests.

Tests can be run for a single protocol or all supported protocols based on
values in `--protocols` flag. For all protocols, don't set the flag. For a
single protocol, set the flag to the protocol you want to test. Current
supported protocols are: `near`, `icp`, `ethereum`

Build application:

```bash
./apps/kv-store/build.sh
```

Prepare Protocols smart contracts:
```bash
cd ../scripts
./download-contracts.sh
```

Move created contracts folder to root level

For testing ICP contracts you will need to deploy ICP devnet:

```bash
/scripts/icp/deploy_devnet.sh
```


Build binaries:

```bash
cargo build -p merod
cargo build -p meroctl
```

Example of running the e2e tests for all supported protocols:

```bash
cargo run -p e2e-tests -- --input-dir ./e2e-tests/config --output-dir ./e2e-tests/corpus --merod-binary ./target/debug/merod --meroctl-binary ./target/debug/meroctl
```

Example of running the e2e tests for multiple protocols (ICP and Ethereum in this
case):

```bash
cargo run -p e2e-tests -- --input-dir ./e2e-tests/config --output-dir ./e2e-tests/corpus --merod-binary ./target/debug/merod --meroctl-binary ./target/debug/meroctl --protocols icp ethereum
```

Useful env vars for debugging:

- `RUST_LOG=debug` - enable debug logs
- `RUST_LOG=near_jsonrpc_client=debug` - or more specific logs
- `NEAR_ENABLE_SANDBOX_LOG=1` - enable near sandbox logs
- `NO_COLOR=1` - disable color output

### Test Design Best Practices

**Wait Durations**:
- `consensus`: 10000ms (5000ms * 2 rounds) - For nodes to join and subscribe
- `broadcast`: 5000ms - For gossipsub propagation and delta application
- Use `retries` and `intervalMs` for operations that depend on async sync

**Expected Handler Behavior**:
```json
{
  "expectedResultJson": 0,
  "description": "Inviter doesn't execute handlers (gossipsub doesn't echo to sender)"
}
```
- **Inviter**: Does NOT execute handlers (gossipsub doesn't echo back to sender)
- **Invitees**: Execute handlers for each delta received via gossipsub
- **Counter accumulates**: Each received delta increments handler execution count on invitees

**Target Options**:
- `"inviter"` - Only the node that created the context
- `"invitees"` - All nodes that joined the context
- `"allMembers"` - All nodes (inviter + invitees)

### Debugging Failed Tests

**Common Issues**:

1. **"Uninitialized" errors** ✅ FIXED
   - **Was**: Invitees querying other uninitialized nodes
   - **Now**: Smart peer selection finds nodes with state
   - **Check**: `RUST_LOG=debug` logs show "Found peer with state"

2. **Timeout on queries**:
   - **Symptom**: Test retries exhaust, still fails
   - **Cause**: Nodes not subscribed or deltas not propagating
   - **Check**: Logs for "Subscribed to context" and "Broadcast delta"

3. **State mismatch**:
   - **Symptom**: Expected value X, got Y
   - **Cause**: DAG fork not resolved or handler count off
   - **Check**: Verify all nodes have same `root_hash` in logs

**Analyzing Logs**:
```bash
# Check subscription timing
grep -r "Subscribed to context" e2e-tests/corpus/logs/

# Check delta propagation
grep -r "Broadcasting state delta" e2e-tests/corpus/logs/

# Check if deltas received
grep -r "Received state delta\|Matched StateDelta" e2e-tests/corpus/logs/

# Check peer selection for uninitialized nodes
grep -r "Found peer with state\|selecting peer with state" e2e-tests/corpus/logs/

# Check for divergence
grep -r "DIVERGENCE DETECTED\|Different root hash" e2e-tests/corpus/logs/
```

### Test Results History

**Latest Run**: October 26, 2025
- ✅ **kv-store-test**: PASSED - All sync working correctly
- ✅ **demo-blockchain-integrations**: PASSED - Proposals and messages sync
- ✅ **kv-store-with-handlers**: PASSED - Handler counts correct (after fix)
- ❌ **open-invitation**: FAILED - 500 Internal Server Error (unrelated to sync)

**Key Metrics**:
- **0% "Uninitialized" errors** (down from 100% before peer selection fix)
- **Invitees successfully bootstrap** from nodes with state
- **Handler execution counts** match expected values
- **State consistency** across all nodes after sync
