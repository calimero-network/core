# E2e tests

Binary crate which runs e2e tests for the merod node.

## Usage

First build apps, contracts, and mero binaries. After that run the e2e tests.

Example of running the e2e tests:

```bash
./apps/kv-store/build.sh

./contracts/near/context-proxy/build-test-deps.sh
./contracts/icp/context-proxy/build_contracts.sh

cargo build -p merod
cargo build -p meroctl

export NO_COLOR=1 # Disable color output for merod logs
cargo run -p e2e-tests -- --input-dir ./e2e-tests/config --output-dir ./e2e-tests/corpus --merod-binary ./target/debug/merod --meroctl-binary ./target/debug/meroctl
```

Useful env vars for debugging:

- `RUST_LOG=debug` - enable debug logs
  - `RUST_LOG=near_jsonrpc_client=debug` - or more specific logs
- `NEAR_ENABLE_SANDBOX_LOG=1` - enable near sandbox logs
- `NO_COLOR=1` - disable color output
