# cargo mero

A Cargo subcommand for building applications on the Calimero network.

## Features

- Scaffold new Calimero applications
- Build Calimero apps to WASM

## Installation

### From Source

From the workspace root:

```sh
cargo install --path crates/cargo-mero
```

Make sure `~/.cargo/bin` is in your `PATH` so that `cargo mero` is available as a subcommand.

## Usage

### Create a new application

```sh
cargo mero new <app-name>
```

### Build your application

```sh
cd <app-name>
cargo mero build
```

#### Pass through additional cargo build arguments

```sh
cargo mero build --verbose
cargo mero build --features feature1,feature2
```
