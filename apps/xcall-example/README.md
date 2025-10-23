# Cross-Context Call Example

This example demonstrates how to use cross-context calls (xcall) in Calimero applications.

## Overview

This application shows how one context can call functions on another context. It implements:
- **send_greeting**: Sends a greeting to another context by calling its `receive_greeting` function
- **receive_greeting**: Receives and stores greetings from other contexts
- **get_messages**: Retrieves all received messages

## How it works

1. Deploy this application to two different contexts (Context A and Context B)
2. From Context A, call `send_greeting` with Context B's ID and a message
3. The xcall will execute `receive_greeting` on Context B after Context A's execution completes
4. Context B will store the message, which can be retrieved using `get_messages`

## Example Usage

```bash
# Deploy to Context A
meroctl --node-name node1 context deploy --application-id <app-id> --context-id <context-a-id>

# Deploy to Context B  
meroctl --node-name node1 context deploy --application-id <app-id> --context-id <context-b-id>

# Send a greeting from Context A to Context B
meroctl --node-name node1 context mutate \
  --context-id <context-a-id> \
  --method send_greeting \
  --args-json '{"target_context": "<context-b-id-hex>", "message": "Hello from Context A!"}'

# Check messages received by Context B
meroctl --node-name node1 context query \
  --context-id <context-b-id> \
  --method get_messages
```

## Key Concepts

- **xcall**: Cross-context calls are queued during execution and executed locally after the main execution completes
- **No Broadcasting**: Unlike events, xcalls are not broadcast to other nodes - they execute locally
- **Parameters**: Function parameters should be JSON-encoded to match the target function's expected input
- **Context ID**: The 32-byte context ID is required to specify which context to call

## Testing

This application includes an automated workflow for end-to-end testing across multiple nodes.

### How the Test Works

1. **Node 1** creates both Context A and Context B
2. **Node 2** creates two identities and joins both contexts (so both nodes have both contexts)
3. **Node 1** calls a method on Context A, which makes an xcall to Context B
4. The xcall finds an owned member of Context B and executes the call locally (on Node 1)
5. The execution on Context B produces a state delta
6. The state delta is broadcast to the network
7. **Node 2** receives and syncs the Context B state change
8. Both nodes now have the same state for Context B

### What the Workflow Tests

- Cross-context calls (xcall) between contexts on the same node
- State delta generation from xcall execution
- State synchronization across nodes
- Bi-directional xcalls (A→B and B→A)
- Multiple xcalls
- Message clearing functionality

### Running the Tests

```bash
# Build the application first
./build.sh

# Run the workflow using merobox
merobox bootstrap run workflows/xcall-example.yml
```

The workflow verifies that:
- XCalls execute locally and produce state deltas
- State deltas from xcalls are broadcast and synced correctly
- Both nodes maintain consistent state for all contexts

