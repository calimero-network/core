# Calimero Node

- [Introduction](#introduction)
- [Core components](#core-components)
  - [Store](#store)
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
    Node : +Store store
    Node : +ContextManager ctx_manager
    Node : +NetworkClient network_client
    Node : +Sender[NodeEvents] node_events
    Node: +handle_event()
    Node: +handle_call()
```

### Store

`Store` is a struct that is used to interact with the underlying storage. The
node interacts with the store in two ways:

- It passes the store as a `Storage` trait to the runtime when running a method
  on the application WASM.
  - Checkout `runtime_compat` module for more information on interoperability
    between runtime and store.
- It directly interacts with the store to commit changes performed by the
  application WASM, to store ContextTransaction and to update ContextMeta to the
  latest hash.

Important structs in the store are:

- `ContextTransaction`:
  https://github.com/calimero-network/core/blob/37bd68d67ca9024c008bb4746809a10edd8d9750/crates/store/src/types/context.rs#L97
- `ContextMeta`:
  https://github.com/calimero-network/core/blob/37bd68d67ca9024c008bb4746809a10edd8d9750/crates/store/src/types/context.rs#L16

## Core flows

### Catchup

The catchup process is initiated by the `ClientPeer` by opening a stream to the
`ServerPeer`.

Once the connection is established, the `ClientPeer` requests the application
information from the ContextConfig contract. If the application blob id has
changed, the `ClientPeer` attempts to fetch new application blob and store it in
the store. Depending on the application source, the `ClientPeer` either fetches
the application blob from the remote BlobRegistry or requests the `ServerPeer`
to send the application blob.

After the application is updated, the `ClientPeer` requests the transactions
from the `ServerPeer`. `ServerPeer` collects executed and pending transactions
from the given hash to the latest transaction. The transactions are sent in
batches to the `ClientPeer` which applies the transactions to the store.

Following diagram depicts the catchup process. The `ClientPeer` in this scenario
is regular peer (not coordinator). The `ServerPeer` can be either regular peer
or coordinator.

```mermaid
sequenceDiagram
    ClientPeer->>+ServerPeer: OpenStream
    Activate ClientPeer

    ClientPeer->>+ContextConfigContract: GetApplication(ctx_id)
    ContextConfigContract-->-ClientPeer: Application

    ClientPeer->>ClientPeer: GetApplication(app_id)

    opt If ApplicationId has changed or Blob is not present

    alt ApplicationSource == Path
    ClientPeer->>ServerPeer: Send [ApplicationBlobRequest(app_id)]
    ServerPeer->>ClientPeer: Send [ApplicationBlobSize]
    loop
    ServerPeer->>ClientPeer: Send [ApplicationBlobChunk]
    ClientPeer->>ClientPeer: blobs.Put(app)
    end

    end

    ClientPeer->>ClientPeer: store.PutApplication(app)

    end

    ClientPeer->>ServerPeer: Send [TransactionsRequest(ctx_id, hash)]
    ServerPeer->>ServerPeer: CollectExecutedAndPendingTransactions(ctx_id, hash)

    loop
    ServerPeer->>-ClientPeer: Send [TransactionsBatch]
    ClientPeer->>ClientPeer: ApplyBatch
    loop For Transaction in Batch
    alt Transaction.Status == Executed
    ClientPeer->>ClientPeer: ExecuteTransaction

    else Transaction.Status == Pending
    ClientPeer->>ClientPeer: ExecuteTransaction
    end

    end
    end

    Deactivate ClientPeer
```
