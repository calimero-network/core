# Cross-Context Call Example

This example demonstrates how to use cross-context calls (xcall) in Calimero applications using a simple ping-pong pattern.

## Overview

This application shows how one context can call functions on another context. It implements:
- **ping**: Sends a ping to another context by calling its `pong` function via xcall
- **pong**: Receives a pong from another context and increments a counter
- **get_counter**: Returns the current counter value
- **reset_counter**: Resets the counter to zero

## How it works

1. Deploy this application to two different contexts (Context A and Context B)
2. From Context A, call `ping` with Context B's ID
3. The xcall will execute `pong` on Context B after Context A's execution completes
4. Context B will increment its counter, which can be retrieved using `get_counter`

## Example Usage

```bash
# Deploy to Context A
meroctl --node-name node1 context deploy --application-id <app-id> --context-id <context-a-id>

# Deploy to Context B  
meroctl --node-name node1 context deploy --application-id <app-id> --context-id <context-b-id>

# Send a ping from Context A to Context B
meroctl --node-name node1 context mutate \
  --context-id <context-a-id> \
  --method ping \
  --args-json '{"target_context": "<context-b-id-base58>"}'

# Check the counter on Context B
meroctl --node-name node1 context query \
  --context-id <context-b-id> \
  --method get_counter
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
- State delta generation from xcall execution (counter increments)
- State synchronization across nodes
- Bi-directional ping-pong (A→B and B→A)
- Multiple pings incrementing the counter correctly
- Counter state consistency across nodes

### Running the Tests

```bash
# Build the application first
./build.sh

# Run the workflow using merobox
merobox bootstrap run workflows/xcall-example.yml
```

The workflow verifies that:
- Pings sent via xcall execute locally and increment the receiver's counter
- Counter state changes are broadcast and synced correctly to all nodes
- Both nodes maintain consistent counter state for all contexts
- Multiple pings correctly accumulate in the counter

