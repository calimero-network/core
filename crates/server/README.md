# Node Server

- [Introduction](#introduction)
  - [Admin API](#1-admin-api)
  - [JSON rpc](#2-json-rpc)
  - [Websocket](#3-websocket)
- [Node Server Workflows](#node-server-workflows)
  - [Client Login Workflow](#client-login-workflow)
  - [JSON rpc Workflow](#json-rpc-workflow)
  - [Websocket Workflow](#websocket-workflow)
- [Admin API endpoints](#admin-api-endpoints)
  - [Protected Routes](#protected-routes)
  - [Unprotected Routes](#unprotected-routes)
- [JSON rpc endpoint](#json-rpc-endpoint)
- [Websocket endpoints](#websocket-endpoints)
- [Examples](#examples)

## Introduction

Node Server is a component in node that facilitates node administration and
enables communication with the logic of an application (loaded wasm) in
participating contexts.

Node Server component is split into 3 parts:

1.  [Admin API](https://github.com/calimero-network/core/blob/feat-admin_api_docs/crates/server/src/admin/service.rs)
2.  [JSON rpc](https://github.com/calimero-network/core/blob/feat-admin_api_docs/crates/server/src/jsonrpc.rs)
3.  [Websocket](https://github.com/calimero-network/core/blob/feat-admin_api_docs/crates/server/src/ws.rs)

### 1. Admin API

The Admin API component of the Node Server exposes API for connection with the
node and its functionalities. It is primarily utilized by the Admin Dashboard to
query and manage various aspects of the node, including:

- Identity information
- Root keys
- Client keys
- Installed applications
- Started contexts

**Data Querying**: The Admin API allows the Admin Dashboard to fetch important
data from the node, such as identity details, root and client keys, and
information about installed and active applications.

**Application Management**: The API provides functionalities for managing
applications and contexts, allowing for the installation and uninstallation of
applications and starting contexts.

**Key Management**: Administrators can manage root and client keys through the
API, ensuring secure access and control over the node.

**Authentication**: The Admin API facilitates user authentication via selected
wallets, currently supporting MetaMask and NEAR networks. Authentication details
will be explained in later sections.

**Integration with Web Applications**: The authentication mechanism is also used
by web applications designed for applications (loaded wasm) in participating
context, ensuring secure and authenticated access.

### 2. JSON rpc

The JSON rpc component of the Node Server facilitates communication between the
clients and the context. This allows seamless interaction and data management
for applications.

The JSON rpc interface provides two primary methods:

- Query
- Mutate

#### Query Method

The `Query` method retrieves data from the application in participating context.
For instance, in the Only Peers forum application, posts and comments stored in
the application's storage can be queried using the JSON rpc interface. This
enables users to fetch and display content from the forum.

#### Mutate Method

The `Mutate` method allows modification of the application's data in
participating context. For example, in the Only Peers forum application, users
can create new posts or comments. The Mutate method updates the application's
storage with these new entries, facilitating dynamic content creation and
interaction within the application.

### 3. Websocket

The WebSocket is used for subscribing to and unsubscribing from certain context
running in the Node Server. Defined handlers manage subscription states for
WebSocket connections, allowing clients to receive updates about specific
contexts they are interested in. WebSocket handlers are essential for managing
real-time subscriptions within the Node Server. They allow clients to
dynamically subscribe to and unsubscribe from updates about various application
contexts.

#### Subscription Handling:

Websocket handles requests to subscribe to specific contexts and send responses
back to the client with the subscribed context IDs.

#### Unsubscription Handling:

Websocket handle requests to unsubscribe from specific contexts and send
responses back to the client with the unsubscribed context IDs.

## Node Server Workflows

### Client Login Workflow

```mermaid
sequenceDiagram
    title Client Login Workflow

    participant User
    participant Admin Dashboard / Application
    participant Crypto Wallet
    participant Admin API
    participant Node

    User->>Admin Dashboard / Application: login
    Admin Dashboard / Application->>Admin API: API call to request-challenge endpoint
    Admin API-->>Admin Dashboard / Application: Return challenge object
    Admin Dashboard / Application->>Crypto Wallet: Request to sign received challenge
    Crypto Wallet-->>User: Request to sign challenge
    User->>Crypto Wallet: Sign challenge
    Crypto Wallet-->>Crypto Wallet: Sign challenge
    Crypto Wallet-->>Admin Dashboard / Application: Signed challenge
    Admin Dashboard / Application-->>Admin Dashboard / Application: Create auth headers
    Admin Dashboard / Application->>Admin API: call add-client-key endpoint with signed challenge + auth header
    Admin API->>Admin API: Verify auth headers
    Admin API->>Admin API: Verify signature
    Admin API->>Node: Check if any root key are stored
    alt No root keys are stored
        Admin API->>Node: Save root key
    else Root key exists
        Admin API->>Node: Save client key
    end
    Node->>Admin API: Key Stored response
    Admin API->>Admin Dashboard / Application: Login successful response
    Admin Dashboard / Application-->>Admin Dashboard / Application: Authorise user
```

### JSON rpc Workflow

```mermaid
sequenceDiagram
    title JSON rpc Workflow

    participant User
    participant Client
    participant JSON rpc
    participant Node

    User->>Client: Query/Mutate action
    Client->>JSON rpc: Request (query/mutate)
    JSON rpc-->>JSON rpc: Processes request
    JSON rpc->>Node: Request for JSON rpc action
    Node->>Node: Perform action (query/mutate)
    Node-->>JSON rpc: Response
    JSON rpc-->>Client: Response for request
```

### Websocket Workflow

```mermaid
sequenceDiagram
    title WebSocket Subscription Workflow

    participant Client
    participant WebSocket
    participant Node

    Client->>WebSocket: Subscribe/Unsubscribe request
    WebSocket->>Node: Handle subscribe/unsubscribe
    Node-->>WebSocket: Subscription response
    WebSocket-->>Client: Subscription messages (context application updates)
```

## Admin API endpoints

The Admin API endpoints are split into protected and unprotected routes, where
protected routes require authentication.

**Base path**: `/admin-api`

### Protected Routes

These routes require authentication using auth headers. Auth headers are
generated using `createAuthHeader` function from the `calimero sdk` library.

Parts of the Auth Headers

1.  `wallet_type`: Specifies the type of wallet used (e.g., NEAR).
2.  `signing_key`: Encoded public key used for signing the request.
3.  `signature`: Encoded signature generated from the payload hash.
4.  `challenge`: Encoded hash of the payload, serving as a challenge.
5.  `context_id`: Context identifier for additional request context. Optional
    for Admin Dashboard but mandatory for applications.

**1. Create Root Key**

- **Path**: `/root-key`
- **Method**: `POST`
- **Description**: Creates a new root key in the node.

**2. Install Application**

- **Path**: `/install-application`
- **Method**: `POST`
- **Description**: Installs a new application in the node.

**3. List Applications**

- **Path**: `/applications`
- **Method**: `GET`
- **Description**: Lists all installed applications in the node.

**4. Fetch DID**

- **Path**: `/did`
- **Method**: `GET`
- **Description**: Fetches the DID (Decentralized Identifier) of the node.

**5. Create Context**

- **Path**: `/contexts`
- **Method**: `POST`
- **Description**: Creates a new context.

**6. Delete Context**

- **Path**: `/contexts/:context_id`
- **Method**: `DELETE`
- **Description**: Deletes a specific context by ID.

**7. Get Context**

- **Path**: `/contexts/:context_id`
- **Method**: `GET`
- **Description**: Retrieves details of a specific context by ID.

**8. Get Context Users**

- **Path**: `/contexts/:context_id/users`
- **Method**: `GET`
- **Description**: Lists users associated with a specific context.

**9. Get Context Client Keys**

- **Path**: `/contexts/:context_id/client-keys`
- **Method**: `GET`
- **Description**: Lists client keys for a specific context.

**10. Get Context Storage**

- **Path**: `/contexts/:context_id/storage`
- **Method**: `GET`
- **Description**: Retrieves storage information for a specific context.

**11. List Contexts**

- **Path**: `/contexts`
- **Method**: `GET`
- **Description**: Lists all contexts.

**12. Delete Auth Keys**

- **Path**: `/identity/keys`
- **Method**: `DELETE`
- **Description**: Deletes all root and client keys.

### Unprotected Routes

These routes do not require authentication.

**1. Health Check**

- **Path**: `/health`
- **Method**: `GET`
- **Description**: Checks the health of the API.

**2. Request Challenge**

- **Path**: `/request-challenge`
- **Method**: `POST`
- **Description**: Requests a challenge for authentication.

**3. Add Client Key**

- **Path**: `/add-client-key`
- **Method**: `POST`
- **Description**: Adds a new client key.

**4. Install Dev Application**

- **Path**: `/dev/install-application`
- **Method**: `POST`
- **Description**: Installs a development application.

**5. Manage Dev Contexts**

- **Path**: `/dev/contexts`
- Methods: `GET`, `POST`
- **Description**: Lists (`GET`) and creates (`POST`) development contexts.

**6. List Dev Applications**

- **Path**: `/dev/applications`
- **Method**: `GET`
- **Description**: Lists all development applications.

## JSON rpc endpoint

The JSON-rpc server endpoint is structured to handle various request types.

**Base path**: `/jsonrpc`

**1. Handle JSON-rpc Request**

- **Path**: `/jsonrpc`
- **Method**: `POST`
- **Description**: Handles incoming JSON-rpc requests, which can be `query` or
  `mutate` requests, processes them, and returns the appropriate response.

## Websocket endpoints

The WebSocket, accessible at /ws, allows clients to dynamically subscribe to and
unsubscribe from real-time updates about specific contexts within the Node
Server.

**1. Handle WebSocket Request**

- **Path**: `/ws`
- **Method**: `GET`
- **Description**: Handles incoming WebSocket requests, which can be subscribe
  or unsubscribe requests, processes them, and returns the appropriate response.

## Examples

Examples of Node Server usage can be found within the
[Admin Dashboard](https://github.com/calimero-network/admin-dashboard) and the
[Only Peers example](https://github.com/calimero-network/only-peers-client)
application. All communication with the node is exposed through
[calimero sdk](https://github.com/calimero-network/core/tree/feat-admin_api_docs/packages/calimero-sdk)
library.
