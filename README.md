# core
Calimero 2.0

# Run
## Setup number of peers (here 3)
```
./crates/node/gen_localnet_configs.sh 3
```

## Turn on debug mode (optional)
```
export RUST_LOG=debug
```

### Testing

#### First, compile the only-peers and kv-store applications

```console
$ ./apps/only-peers/build.sh
$ ./apps/kv-store/build.sh                                                                                        
```

#### Create a data folder for all configs

```console
$ mkdir data
```

#### Spin up a coordinator node

```console
$ cargo run -p calimero-node -- --home data/coordinator init --server-port 2427
    Finished dev [unoptimized + debuginfo] target(s) in 0.20s
     Running `target/debug/calimero-node --home data/coordinator init`
2024-02-28T20:02:57.715257Z  INFO calimero_node::cli::init: Generated identity: PeerId("12D3KooWCiyHe2yeu53qSyRs4g9sTGwgEPjR8iGdi4XG5iv11TgH")
2024-02-28T20:02:57.725088Z  INFO calimero_node::cli::init: Initialized a chat node in "data/coordinator"

$ cargo run -p calimero-node -- --home data/coordinator run --node-type coordinator
```

#### Spin up node 1

```console
$ cargo run -p calimero-node -- --home data/node1 init --server-port 2428 --swarm-port 2528
    Finished dev [unoptimized + debuginfo] target(s) in 0.20s
     Running `target/debug/calimero-node --home data/node1 init`
2024-02-28T20:02:57.715257Z  INFO calimero_node::cli::init: Generated identity: PeerId("12D3KooWHJMh2hv9wai6UqPoHf5jED2gNaUbTTx6ZThAUqroCgtF")
2024-02-28T20:02:57.725088Z  INFO calimero_node::cli::init: Initialized a chat node in "data/node1"

$ cargo run -p calimero-node -- --home data/node1 run
```

```
Check if config file has set correct port in all places. If not, update it per given port value.
```

#### Spin up node 2

```console
$ cargo run -p calimero-node -- --home data/node1 init --server-port 2429 --swarm-port 2529
    Finished dev [unoptimized + debuginfo] target(s) in 0.20s
     Running `target/debug/calimero-node --home data/node2 init`
2024-02-28T20:02:57.715257Z  INFO calimero_node::cli::init: Generated identity: PeerId("12D3KooWHDWr9mCgZiXQXKDsMjWgDioAt9mVHAKEuYUuSKtYdv75")
2024-02-28T20:02:57.725088Z  INFO calimero_node::cli::init: Initialized a chat node in "data/node2"

$ cargo run -p calimero-node -- --home data/node2 run
```

```
Important!!
If you receive error
"message": "guest panicked: panicked at apps/only-peers/src/code_generated_from_calimero_sdk_macros.rs:41:44: Failed to read app state.",
that means that storage is empty. Before fetching any data, create new post.
```

All sessions will fall into interactive mode

```console
Usage: [call|peers|pool|gc|store] [args]

> call <method> <JSON args>

Call a method on the app with the provided JSON args

> peers

Show a count of connected peers

> pool

Show the transaction pool

> gc

Evict all transactions in the transaction pool that are awaiting confirmation

> store

Print the DB state
```

Example

#### From Peer 1

```console
> call set { "key": "name", "value": "Adam Smith" }
 │ Sent Transaction! Hash("DWSBHcnDnNVkQTf5xha891kfQvXyQt6WMhyReghcLW5A")
 │ Hash("DWSBHcnDnNVkQTf5xha891kfQvXyQt6WMhyReghcLW5A")
 │   (No return value)
 │   Logs:
 │     > Setting key: "name" to value: "Adam Smith"
> call get { "key": "name" }
 │ Sent Transaction! Hash("9Y5jZVsmEs1P74qhi2uJ82jr7WFFUCg1X6TvHtoLo45W")
 │ Hash("9Y5jZVsmEs1P74qhi2uJ82jr7WFFUCg1X6TvHtoLo45W")
 │   Return Value:
 │     > "Adam Smith"
 │   Logs:
 │     > Getting key: "name"
```

#### From Peer 2

```console
> call get { "key": "name" }
 │ Sent Transaction! Hash("EFthDcmVbpevfYw1T7WfQ75tY7PHV7DVKieRNFa2uanh")
 │ Hash("EFthDcmVbpevfYw1T7WfQ75tY7PHV7DVKieRNFa2uanh")
 │   Return Value:
 │     > "Adam Smith"
 │   Logs:
 │     > Getting key: "name"
> call set { "key": "name", "value": "Adam Smitten" }
 │ Sent Transaction! Hash("7eU6aJHgB4rpZn8oV7VbWMxERDDKMCP2Ao2yj5G96WZD")
 │ Hash("7eU6aJHgB4rpZn8oV7VbWMxERDDKMCP2Ao2yj5G96WZD")
 │   (No return value)
 │   Logs:
 │     > Setting key: "name" to value: "Adam Smitten"
> call get { "key": "name" }
 │ Sent Transaction! Hash("86Rfq6zEpjDSMjXFfxwmLLscHob9ZBtJEwvhwEDptjhM")
 │ Hash("86Rfq6zEpjDSMjXFfxwmLLscHob9ZBtJEwvhwEDptjhM")
 │   Return Value:
 │     > "Adam Smitten"
 │   Logs:
 │     > Getting key: "name"
```
