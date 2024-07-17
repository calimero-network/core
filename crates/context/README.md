# Calimero Contexts

- [Context Lifecycle](#context-lifecycle)
- [Context State](#context-state)
- [State Mutations](#state-mutations)
- [Context Membership](#context-membership)
  - [Invitations](#invitations)
  - [Leaving](#leaving)
- [Encryption](#encryption)
- [Application Upgrade](#application-upgrade)
- [Context Deletion](#context-deletion)

A context in Calimero is an instance of a deployed application where members share a synchronized state.

Contexts are identified by a 32-byte key, which is randomly determined when each context is created. As of now, this key is not necessarily cryptographically secure, nor is it a dependency of cryptographic operations, but it is unique enough to mitigate collisions. Worth noting that this is likely to change in the future.

All state mutations within a context performed by members will be broadcasted to all other members.

## Context Lifecycle

Following the creation of a context, the only member is the creator. The creator can invite other members to join the context, and members can leave the context at any time.

## Context State

The state of a context is a key-value store, where the key is a 32-byte identifier and the value is a byte array. The state is synchronized across all members of the context, since all state mutations are broadcasted to all members.

## State Mutations

A state mutation is an operation that changes the state of the context. As of the time of writing, state mutations are represented as transactions that define the application method to be called, and it's input (arguments). We make no assumptions about the application, nor it's inputs and outputs, as long as it's clients agree on the schema & serialization method.

All members of the context, following the reception of a transaction, will execute the transaction on their local copy of the state to advance it to the new state.

The unfortunate downside of this approach is that we're unnecessarily executing the same transaction on all nodes without using that information to achieve trustless consensus. We're currently working on a solution to this problem, which reimagines consensus.

## Context Membership

The membership of a context is a list of member identifiers. Each member is identified by a 32-byte key, which represents the public key of the member.

### Invitations

Alice wants to invite Bob to join the context. Alice shares the context ID with Bob. Bob can now make a catchup request, which will be responded to by Alice. Eventually, Bob receives all transactions required to make up the complete state, at which point he is considered a full member of the context and can proceed to make mutations.

### Leaving

Bob wants to leave a context. He deletes the context from his local storage and no longer tracks the state of the context although the context still exists and other members can continue to make mutations. Bob is no longer considered a member of the context.

## Encryption

As of the time of writing, all messages are sent in plaintext. But we're currently working on encrypting all messages using distinct, yet, deterministic keys by employing the double-ratchet algorithm, which provides forward secrecy, and post-compromise security. We'll revise this document once this, among other features, is implemented.

## Application Upgrade

This is not implemented at this time.

## Context Deletion

There's no way to "delete a context" for all members outside of the scope of all members [leaving it](#leaving).
