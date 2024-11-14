# E2e tests

Binary crate which runs e2e tests for the merod node.

## Usage

Build the merod and meroctl binaries and run the e2e tests with the following
commands:

```bash
cargo build -p merod
cargo build -p meroctl

cargo run -p e2e-tests -- --input-dir ./e2e-tests/config --output-dir ./e2e-tests/corpus --merod-binary ./target/debug/merod --meroctl-binary ./target/debug/meroctl
```
