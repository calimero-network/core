# Review guidance for calimero-network/core

This is a distributed p2p system: CRDT state, a causal DAG, consensus/governance,
a WASM runtime, and node-to-node crypto. Weight these as high-severity when reviewing:

- Determinism: consensus and state-transition paths must be deterministic across nodes.
  Flag HashMap/HashSet iteration order, system time, RNG, or float math on any path that
  feeds replicated state or the DAG.
- CRDT correctness: merges must stay commutative, associative, and idempotent. Flag changes
  that could break convergence or make merge order-dependent.
- DAG invariants: causal ordering and cycle prevention; never assume a total order.
- Untrusted input: bytes from peers are hostile. Deserialization must bound-check and never
  panic on malformed input, since a panic on the network path is a denial of service.
- Crypto: constant-time comparison for secrets and MACs, correct nonce/key lifecycle, and no
  key material in logs or error messages.
- Authorization: capability and governance checks must not be bypassable by message reordering
  or a crafted payload.
