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
