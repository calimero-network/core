# CORE CLI - Command Line Interface for a P2P Network

The core-cli is a command-line interface (CLI) tool designed to interact with a peer-to-peer (P2P) network. It provides a set of commands to join the network, start application sessions, manage cryptographic key pairs, and perform other network-related operations. This tool is intended to be used in a decentralized network environment, where users can interact with various applications and peers.

### Prerequisites

Before using core-cli, ensure that you have the following prerequisites installed on your system:

Node.js (v16+): This tool is built with Node.js and requires it to be installed on your machine.

### Overview

## Overview

_Click on a command for more information and examples._

| Command                                                  | Description                                              |
| -------------------------------------------------------- | -------------------------------------------------------- |
| [`core-cli login`](#browser-login)                       | stores identification data locally                       |
| [`core-cli join`](#join-the-p2p-network)                 | join P2P network and make your Node discoverable         |
| [`core-cli start-session`](start-an-application-session) | start Application session                                |
| [`core-cli add-keypair`](#import-raw-key-pairs)          | import identification key pairs from file or JSON string |
| [`core-cli get-nodes`](#list-all-nodes)                  | list nodes available in the network                      |
| [`core-cli get-apps`](#list-all-apps)                    | list applications available in the network               |

## Installation

To install core-cli, follow these steps:

Clone the repository to your local machine:

```bash
$: git clone https://github.com/calimero-is-near/cali2.0-experimental.git
```

Navigate to the project directory:

```bash
$: cd core/client
```

Install dependencies using pnpm:

```bash
$: pnpm install
```

Build project - build output located in dist folder

```bash
$: pnpm build

> core-cli@0.0.1 build .../core/client
> npx tsc
```

Test CLI tool

```bash
$: node src/index.js help or pnpm dev help

core-cli [command]

Commands:
  core-cli join           join P2P network on specific address
  core-cli start-session  Start an Application Session
  core-cli add-keypair    Support for importing raw key pairs
  core-cli login          Support for browser login
  core-cli get-nodes      List all nodes
  core-cli get-apps       List all apps

Options:
  --version  Show version number                                       [boolean]
  --help     Show help                                                 [boolean]

```

Make core-cli globally available

```bash
$: npm install -g

> added 1 package in 121ms
```

## Usage

core-cli provides several commands that you can use to interact with the P2P network. Here are some of the main commands and their usage:

### `Join the P2P Network`

```bash
$: core-cli join --address <address> [--port <port>] [--token <token>]

> Joining network at address: 127.0.0.1:3000
```

- --address or -a: Specifies the network address to join (required).

- --port or -p: Specifies the port number to connect to (optional, default is 3000).

- --token or -t: Provides an authentication token if required (optional).

### `Start an Application Session`

```bash
$: core-cli start-session --app <appName> [--config <configFilePath>]

> Starting Application Session for: ChatApplication
```

- --app or -a: Specifies the name of the application to start (required).
- --config or -c: Specifies the path to a configuration file (optional).

### `Import Raw Key Pairs`

```bash
$: core-cli add-keypair --keys <keysJsonString> OR --file <filePath>

> Key data imported successfully: KeyData
```

- --keys or -k: Provides the key pair as a JSON string (optional).
- --file or -f: Specifies the path to a JSON file containing the keys (optional).

### `Browser Login`

This command initiates a browser-based login process. Follow the instructions in the terminal to authorize core-cli on your accounts.

```bash
$: core-cli login

> Please authorize CORE CLI on at least one of your accounts
```

### `List All Nodes`

This command lists all available nodes in the network.

```bash
$: core-cli get-nodes

> Listing all nodes...
>
>
 Node                                                  | Address                                              --------------------------------------------------------------------------
 peer-network-node-1                                   | 179.231.23.1:3000
```

### `List All Apps`

This command lists all available applications in the network and provides information on how to connect and configure them.

```bash
$: core-cli get-apps

> Listing all apps...
>
>
 Application                                                  | Address                                              --------------------------------------------------------------------------
 chat-application                                         | 179.231.23.1:3000
 file-sharing-application                                 | 179.231.23.2:3000
 colaboration-doc-application                             | 179.231.23.3:3000
```
