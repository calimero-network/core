# core
Calimero 2.0


# Run
## Setup number of peers (here 3)
```
./crates/node/gen_localnet_configs.sh 3
```

## Download app wasm and specify wasm_path in config
```
[app]
wasm_path = "./app.wasm"
```

## Turn on debug mode
```
export RUST_LOG=debug
```

## For each node start config
```
cargo run --bin calimero-node -- --home ~/.calimero/node1
cargo run --bin calimero-node -- --home ~/.calimero/node2
cargo run --bin calimero-node -- --home ~/.calimero/node3
```
