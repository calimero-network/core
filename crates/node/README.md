# Calimero Node

- [Introduction](#introduction)
- [Core components](#core-components)
  - [NodeType](#nodetype)
  - [Store](#store)
  - [TransactionPool](#transactionpool)
- [Core flows](#core-flows)
  - [Transaction handling](#transaction-handling)
  - [Coordinator joining ceremony](#coordinator-joining-ceremony)
  - [Catchup](#catchup)

## Introduction

The Node crate is a reference implementation of Calimero protocol.

## Core components

Node struct is the main struct that holds all the components of the node. It is
responsible for handling events from network and calls from server or
interactive CLI.

```mermaid
classDiagram
    Node : +PeerId id
    Node : +NodeType typ
    Node : +Store store
    Node : +TransacitonPool tx_pool
    Node : +ContextManager ctx_manager
    Node : +NetworkClient network_client
    Node : +Sender[NodeEvents] node_events
    Node: +handle_event()
    Node: +handle_call()
```

### NodeType

`NodeType` is an enum that represents the type of the node. It can be either
`Coordinator` or `Peer`.

### Store

TODO: Write about the store and runtime compat layer, link to the store crate

### TransactionPool

`TransactionPool` is a struct that holds all the transactions that are not yet
executed. Transaction pool stores transactions in a `BTreeMap` with the key
being the hash of the transaction. `TransactionPoolEntry` is a struct that holds
the transaction, the sender of a transaction and the outcomen sender channel.

## Core flows

### Transaction handling

TODO: Write about the transaction handling process and draw sequence diagram

### Catchup

Catchup process involves updating the context metadata and the application blob, followed up
by replaying the transactions from the last current executed transaction.
