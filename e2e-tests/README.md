# E2e tests

Binary crate which runs e2e tests for the merod node.

## Usage

First build apps, contracts, and mero binaries. After that deploy the devnet (if
needed) and run the e2e tests.

Tests can be run for a single protocol or all supported protocols based on
values in `--protocols` flag. For all protocols, don't set the flag. For a
single protocol, set the flag to the protocol you want to test. Current
supported protocols are: `near`, `icp`, `stellar`

Build application:

```bash
./apps/kv-store/build.sh
```

NEAR setup:

```bash
./contracts/near/context-config/build.sh
./contracts/near/context-proxy/build.sh
```

ICP setup:

```bash
./contracts/icp/context-config/build.sh
./contracts/icp/context-proxy/build.sh
```

After building the ICP contracts, deploy the ICP devnet:

```bash
./contracts/icp/context-config/deploy_devnet.sh
```

STELLAR setup:

```bash
./contracts/stellar/context-config/build.sh
./contracts/stellar/context-proxy/build.sh
```

After building the Stellar contracts, deploy the Stellar devnet:

```bash
./contracts/stellar/context-config/deploy_devnet.sh
```

In case of Stellar, you will need to set the values shown in the output of the
deploy_devnet.sh script in the config file after deploying the devnet: Replace
the following values in the config file under `stellar` section:
`e2e-tests/config/config.json`:

- `contextConfigContractId` with the value of `CONTRACT_ID`
- `publicKey` with the value of `ACCOUNT_ADDRESS`
- `secretKey` with the value of `SECRET_KEY`

Build binaries:

```bash
cargo build -p merod
cargo build -p meroctl
```

Example of running the e2e tests for all supported protocols:

```bash
cargo run -p e2e-tests -- --input-dir ./e2e-tests/config --output-dir ./e2e-tests/corpus --merod-binary ./target/debug/merod --meroctl-binary ./target/debug/meroctl
```

Example of running the e2e tests for multiple protocols (Stellar and ICP in this
case):

```bash
cargo run -p e2e-tests -- --input-dir ./e2e-tests/config --output-dir ./e2e-tests/corpus --merod-binary ./target/debug/merod --meroctl-binary ./target/debug/meroctl --protocols stellar icp
```

Useful env vars for debugging:

- `RUST_LOG=debug` - enable debug logs
- `RUST_LOG=near_jsonrpc_client=debug` - or more specific logs
- `NEAR_ENABLE_SANDBOX_LOG=1` - enable near sandbox logs
- `NO_COLOR=1` - disable color output
