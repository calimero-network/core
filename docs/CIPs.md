# CIPs

## External Interactions from Calimero Nodes

Goals:

- Third-party system representation
- Preserve privacy
- anonymize votes
- anonymize what is being voted on until finalization (where the effect is to be
  public)
- Allow for fractional representation
- Fractions of context members, down to individual identities, should have
  equivalent external representation

Implementation options:

- Cryptographically-defined threshold groups
- Multisig contracts

Cryptographically-defined threshold groups

    The threshold product of individual signatures represents the group's signature, which is associated with an account on the external network.

    This account owns assets, native balances, and can interact with the network as a single entity.

If the threshold product IS the account on the external network (no contract):

    If the group's membership changes, the associated identity product changes as well.

    This means that the account's ownership of assets, and balances, will have to be moved. And leaves open the room for malicious behavior.

The fix would then be to allow updating the contract, pointing it at the new
thresholded public key.

Multisig contracts:

Each identity signs a unique request to either propose or approve, the multisig
contract after verifying the request and identity membership, keeps track of
signers until the threshold is met.

Anonymization:

It's going to be tricky fully anonymizing the votes (between the voters),
because we need to verify that the vote came from a valid member of the
threshold group.

And the property DSAs gives us is the verifiability component. Otherwise we run
the risk of poisoning the session with invalid signatures.

But at least we can anonymize the proposal data, and the votes, from
third-parties. And the only implementation that gets us here would be
cryptographic threshold groups.

It could be that zero-knowledge proofs could facilitate this, but it would have
to commit to both the identity membership and signature product from the same
identity.

Privacy preservation

In both cases, we have to be careful about privacy preservation. This means, all
the proposals and subsequent votes HAVE to stay local (at least until
finalization).

The proposal data will be replicated across nodes same as state data, inheriting
the same security properties.

Identities within the threshold group will broadcast their partial signatures of
the proposal between themselves, incrementally combining them until the
threshold is reached.

On receipt of a N-1 combination, the final signer will broadcast the final
signature to the network, which will then be used to execute the proposal.

Since we cannot guarantee 2 identities don't approve at the same time for the
finalization, the on-chain contract will have to retain references to all
previously approved proposals.

On finalization of proposal execution, the contract should hold on to the
execution response.

Multi-protocol support

In the case where threshold groups aren't governed by the necessity of contract
deployment, then we can support multiple protocols.

For the most part, it's just identity that matters, and in most protocols, the
public key product of calimero threshold interaction, would suffice.

This means, you don't have to create a context "FOR" Starknet "OR" Near, you
simply create a context, and you can derive accounts on every supported
protocol.

We can still do this with contracts, but context creation would be rather
expensive as it would require the deployment of each protocol's version of the
proxy contract on each protocol's network.

This can also be done on demand, however, programmatically. Say, when an ETH
account is requested, the contract is deployed on the ETH network, and the
account is returned.

---

## Calimero's use of the Actor Model

Fundamental pieces of the protocol architecture are broken up into individual
units of computation, each one able to advance independently of the others.

When necessary, actors interoperate by sending messages to one another. Keeping
the entire system asynchronous.

The design of actors also makes it possible to semantically split up the code
with respect to the different roles that parts of the system play.

Keeping things modular, and more intuitive to reason about.

- Network:
- Messaging: recv, send between peers.
- Discovery: identify, relay circuit, holepunch.

- Runtime (CRDTs afford us the ability to do this without locking, but this
  requires some refactoring effort):
- Task manager{N actors}: maintain an execution pool, and schedule tasks.

- Storage (employ locks either against the column or ranges of rows):
- Blob store{N actors}: store, retrieve, and delete blobs.
- State store{N actors}: store, retrieve, and delete state.

- Node:
- Context Management
- Application Management

End goal here would be to facilitate the ability to scale the network, and the
runtime.

Allowing long-running tasks in the runtime to keep running without blocking
anything, not even the same user of the context.

We can further expand on the task manager at some point, to allow monitoring
stats, prioritizing tasks, and killing them at will.
