# Core Cli Wrapper (crate::merow) 

A CLI wrapper for the Calimero Node that provides a default node configuration file for initializing a node, and quickly running a development environment for testing P2P Calimero Apps. 

## Features

- Custom Node Configuration File 
- Simple Commands to Initialize and Run a Calimero Node 
- Creates a Node Home Directory (if it doesn't already exist)

## Prerequisites
- Rust:  [Official Rust Installation](https://www.rust-lang.org/tools/install)

## Setting up
Clone the project

```bash
  git clone https://github.com/kevinjaypatel/core.git 
```

Change to repo  

```bash
  cd core 
```

Check out cli-wrapper
```bash
  git branch cli-wrapper
```

## Usage

Setup the Default Configuration:  `./crates/merow/config/default.toml` 

```javascript
[coordinator]
name = "coordinator" 
server_port = 2427
swarm_port = 2527
home = "data"

[admin] 
name = "node1" 
server_port = 2428 
swarm_port = 2528
home = "data"
```

Initialize a coordinator   
`$ merow -- init-coordinator` 

Initialize a node   
`$ merow -- init-node` 

Start a running coordinator   
`$ merow -- start-coordinator` 

Start a running node   
`$ merow -- start-node` 


## How to Run (from project root)

### Build the Rust Package
```bash
  cargo build 
```

### Starting up  a Coordinator (same steps apply for Node Configuration)
E.g. Initializes Coordinator (with defaults) 
```bash
  cargo run -p merow -- init-coordinator 
```

Start a running coordinator 
```bash
  cargo run -p merow -- start-coordinator 
```

### Accessing the coordinator via Admin Dashboard
```bash
  http://localhost:<coordinator.server_port>/admin-dashboard/
```

## Roadmap

- Additional commands for `Dev Context` creation and `Peer Invitation`
- Add a boolean flag to the Configuration File for Deploying the Admin Dashboard 
- Multi-node deployment (e.g. node1, node2, ... nodeN)
