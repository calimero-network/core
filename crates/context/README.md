# Calimero Contexts

A context in Calimero is an instance of a deployed application members of which share a synchronized state.

A context is identified by a 32-byte key, which is randomly generated when the context is created. As of now, the key is not necessarily cryptographically secure, nor is it a dependency of cryptographic operations, but it is unique enough to mitigate collisions. Worth noting that this is likely to change in the future.

All operations within a context performed by members will be broadcasted to all other members. This includes:

- State mutations
- Membership changes (invitation, leaving, kickouts)

## Context Lifecycle

Following the creation of a context, the only member is the creator. The creator can invite other members to join the context, and members can leave the context at any time.

## Administrative Operations

Application authors can define gated behaviour for administrative operations, but by default all members have the same rights. Which, depending on the application, can be an undesirable behaviour.

## Context State

The state of a context is a key-value store, where the key is a 32-byte identifier and the value is a byte array. The state is synchronized across all members of the context, and all state mutations are broadcasted to all members.

## State Mutations

A state mutation is an operation that changes the state of the context. As of the time of writing, state mutations are represented as transactions that define the application method to be called, and it's input (arguments). We make no assumptions about the application, nor it's inputs and outputs, as long as it's clients agree on the schema & serialization method.

All members of the context, following the reception of a transaction, will execute the transaction on their local copy of the state to advance it to the new state.

The unfortunate downside of this approach is that we're unnecessarily executing the same transaction on all nodes without using that information to achieve trustless consensus. We're currently working on a solution to this problem, which reimagines consensus.

## Consensus

As of the time of writing, we're using a simple consensus mechanism, where all members of the context must agree on the order of transactions. This is achieved by broadcasting transactions to all members, who queue them up in a pool, waiting for a confirmation from an elected coordinator. This coordinator is responsible for ordering transactions, which ensures that all members of the context eventually reach the same state.

The coordinator also stores the transaction history, and so, can reject transactions that have already been processed as well as ones that don't reference the latest state.

## Context Membership

The membership of a context is a list of member identifiers. Each member is identified by a 32-byte key, which represents the public key of the member.

### Invitations

Bob wants to join a context. He shares his public key with Alice, who is already a member of the context. Alice then broadcasts an invitation to all members of the context, which is made up of Bob's public key. Following this invitation, Alice shares the context ID with Bob. At this point, Bob can make a catchup request, which will be responded to by any member of the context who has some of the state and now knows about Bob. Eventually, Bob receives all transactions required to make up the complete state, at which point he is considered a full member of the context. And can proceed to make mutations.

### Leaving

Bob wants to leave a context. He broadcasts a leave request to all members of the context, which is made up of his public key. Following this request, Bob is removed from the membership list, and all members will stop broadcasting state mutations to Bob.

## Encryption

As of the time of writing, all messages are sent in plaintext. But we're currently working on encrypting all messages using distinct, yet, deterministic keys by employing the double-ratchet algorithm, which provides forward secrecy, and post-compromise security. We'll revise this document once this, among other features, is implemented.

## Application Upgrade

As noted [above](#administrative-operations), application authors can define gated behaviour for administrative operations. This includes the ability to upgrade the application. Subsequent transactions made by members from the old version of the application will be rejected by the new version.

## Context Deletion

There's no way to "delete a context" for all members outside of the scope of all members leaving it.
