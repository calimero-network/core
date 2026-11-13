# Review guidance for calimero-network/core

This is a confidential cloud: each app instance is replicated by its own Raft log
(openraft, `calimero-log`) across attested TDX nodes; a system group (group 1) holds
accounts, organizations, teams, invites, packages and the fleet; clients and peers
reach a node only through the sealed channel (`calimero-sealed`) after checking its
attestation (`calimero_client::attest::verify`); apps are `calimero:app` WASM
components on wasmtime. Weight these as high-severity when reviewing:

- Apply is deterministic: the leader runs the call and logs `{ call, outcome { result,
  diff, events } }`; followers apply the diff and never re-execute. Anything `apply`
  does (system or app machine) must give the same state on every replica from the
  entry alone. Flag a node's clock, RNG, HashMap/HashSet iteration order, local config,
  attestation results or reads of another log (an app's apply never reads system state)
  on an apply path; times come from the entry (`at`, `tsMs`). The leader's run may use
  them; what it decides goes into the entry.
- Stale and racing leaders: a deposed leader may run a call on stale state; its entry
  must commit as a no-op (`base_index` against the app's last entry, `conflict`) and be
  re-run, never applied on the wrong base. Flag work done outside the proposal lock that
  the entry assumes, and side effects (outbound requests, stored bytes) not undone when
  the entry does not commit.
- Old entries and old nodes: a log replays every entry an earlier release wrote, and a
  rolling upgrade mixes releases. A new command or a field that changes what one does is
  a `Feature` in `system::cluster_version::FEATURES`, refused below the cluster version;
  an entry logged before a rule change must apply as it did. A stored or replicated shape
  changes only by raising `STORE_FORMAT`, `SNAPSHOT_FORMAT` or `ENTRY_FORMAT` with a
  reader for the old one.
- Proofs are checked at apply: a decision carries its proof (a passkey consent, an
  operator signature, an invite or API key signature), judged in `system::authorize`
  and spent (`used_proofs`) on every replica, not only by the API that took it. Flag an
  authority check that exists only in a route handler, an identity taken from a request
  field instead of the session, a signature or the attested channel, and membership
  judged on stale system state instead of a fresh read (`sys_fresh`).
- Attestation and the sealed channel: every route but `/attestation`, `/health` and the
  consent page is inside the channel; peers (`internal/*`, Raft) are attested like
  clients and checked as senders and addressees. Flag a quote check that bypasses the
  one shared function, a key not bound to the quote, and data sent to a node whose
  release the app's organization has not approved (`may_send`).
- Untrusted input: callers, peers, registry downloads and app guests are hostile. Every
  input has a named limit constant checked before the costly work; deserialization
  refuses unknown fields and never panics, since a panic on a request or apply path is
  a denial of service (an undecodable entry applies as a no-op).
- WASM components: a guest runs under fuel and memory caps; host functions check what
  the guest asks for (its own files, blobs, runs) and never hand it state, network or
  storage its world does not import. Compute runs on job nodes are judged by signed
  reports, never by trusting the job node's word.
- Crypto: constant-time comparison for secrets and MACs, correct nonce and key
  lifecycle (keys derived per purpose with a label, zeroized when dropped), and no key
  material, tokens or app data in logs or error messages.
