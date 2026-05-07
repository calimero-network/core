# Cross-DAG Authorization & Convergence — Implementation Roadmap

| | |
|---|---|
| **Status** | Roadmap — issue-ready |
| **Date** | 2026-05-07 |
| **Authors** | sandi@calimero.network |
| **Scope** | Governance DAG ↔ State DAG coordination, member removal & leave semantics, convergence detection, eclipse / DoS surface |
| **Related** | [#2233](https://github.com/calimero-network/core/issues/2233), [#2237](https://github.com/calimero-network/core/issues/2237), [#2280](https://github.com/calimero-network/core/pull/2280), [#2284](https://github.com/calimero-network/core/pull/2284), [`docs/adr/0001-shared-storage-concurrent-rotation.md`](../adr/0001-shared-storage-concurrent-rotation.md) |

> **About this document.** Roadmap form. Each entry below is sized for direct conversion into a GitHub issue. Full design rationale (alternatives considered, divergence scenarios, attack analysis) is preserved in the git history of this file — see commit `c29b3565` and `cd5ece66` for the design-doc form.

---

## 1. Problem (one paragraph)

Calimero runs two logically independent DAGs per group — a governance DAG (membership/role ops) and a state DAG (CRDT writes) — and they are not causally cross-referenced. A removed member's writes can be applied on a peer that has not yet observed `MemberRemoved`, then propagate further as legitimate descendants build on them. The wire-format `governance_epoch` field that was meant to bridge the two DAGs is dead (sent as `vec![]`, ignored on receive since #2237 Phase 11.2). The receive-path membership check is partial — only `ReadOnly`/`ReadOnlyTee` are rejected; outright removal silently passes. There is no governance-state-hash on the admin API, so e2e tests fall back to fixed sleeps to confirm governance has propagated. There is no eclipse-resistant bootstrap path; a late joiner that connects to a malicious peer (incl. a removed one) can be fed a stale governance view.

## 2. Key design decisions

These are decided. Don't relitigate inside individual issues.

1. **`governance_position` on every state delta.** A struct `{ group_id, group_state_hash, governance_dag_heads }` embedded in `ContextDagDelta` / `CausalDelta`, replacing the dead `governance_epoch`.
2. **Validity rule is a pure function of `(delta.governance_position, canonical governance DAG)`.** Same answer on every peer, regardless of apply-time or local clock.
3. **Buffer-on-unknown.** Receivers buffer state deltas whose `governance_position` references governance heads not yet observed locally. Decision is deferred until governance catches up.
4. **Forward-only semantic.** Writes from a pre-cut `governance_position` are valid forever, regardless of arrival order or partition path. No retroactive invalidation.
5. **Cut declarations are per-actor signed, not vote-based.** Forced removal: admin embeds an explicit `cut: GovernancePosition` in the signed `MemberRemoved` op. Voluntary leave: leaver embeds their own `governance_position` in `MemberLeft`.
6. **Owner override is the recovery path.** Owner can sign `MemberRestored` to reverse an admin's `MemberRemoved`. Replaces what a quorum design would have offered.
7. **D1 (network-layer deny-list keyed on signer identity) is load-bearing.** Collapses the long-range attack surface to gossip-propagation time of `MemberRemoved`. D2 (time-window), D3 (K0 deprecation), D4 (rate limits) are defense-in-depth.
8. **Bootstrap pins to Owner / TEE peer.** Late joiner dials Owner by peer-id; libp2p handshake authenticates that the responder holds the corresponding private key. TEE peer is a redundancy alternative. Multi-source bootstrap is the fallback when Owner/TEE is unreachable.
9. **Snapshots are scoped to recovery, not long-range defense.** Owner-signed (single-sig). Bound rebuild scope; provide a bootstrap floor for stale-position rejection.
10. **No quorum / M-of-N voting / multisig anywhere.** This is a hard constraint, not a preference. See `out of scope`.
11. **Owner-anchored bootstrap is the durable trust model — light-client / proof-based verification is not a target use case.** Late joiners trust Owner's signed answer (verified via libp2p peer-id auth); we do not need inclusion/exclusion proofs against group state. This drives several downstream decisions, including keeping `compute_group_state_hash` as flat SHA-256 (no Merkle refactor).
12. **Layered hashes, not unified — and reuse one Merkle primitive only where actually needed.** `compute_group_state_hash` (governance, flat SHA-256), `Snapshot::root_hash` (state, existing storage Merkle via `Index<S>`), and `governance_dag_root` (existing) stay independent and update at their own rates. `NamespaceMerkle` (A3) composes them hierarchically using a small reusable Merkle primitive that is **extracted on-demand when A3 lands, not speculatively**. Storage `Index<S>` is left untouched (hot path, well-tested, in-scope independently per #2238).

## 3. Phases & sequencing

| Phase | Items | Goal |
|---|---|---|
| **1 — Quick wins** (parallel) | A1, A2, D1, E2 | Observability + immediate DoS surface reduction. No wire-format changes. |
| **2 — Foundational** (sequential) | B1 → B2 → B3 → C4 | The cross-DAG primitive. After this, validity is well-defined. |
| **3 — Removal flow + recovery** | C1, C2, C3, D2, D3, C5, C6 (parallel where possible) | Deterministic cuts + Owner override + defense-in-depth + bounded rebuild. |
| **4 — Bootstrap & long-tail** | E1, E3, E4, A3, D4 | Eclipse-resistant join + whole-subtree convergence + rate limits. |

---

## 4. Unknowns to resolve before / during implementation

### 4.1 Blocking unknowns — must answer before the listed issue can start

| # | Question | Blocks | Suggested resolution path |
|---|---|---|---|
| **U1** | **Wire-format breaking change strategy.** Versioned protocol message vs. hard cut at a release boundary? How are in-flight deltas authored under the old shape handled during the rollout window? | B1 | Design call; coordinate with release manager. Probably hard cut at a release with explicit upgrade notes. |
| **U2** | **Governance state history storage.** B3 needs to look up "was X a member at state hash H?" — requires retaining a ring buffer of recent group state hashes → membership snapshots. How big? Eviction policy? Replay-from-checkpoint strategy when the lookup target is older than the buffer? | B3 | Discovery as part of B3. Reasonable default: ring buffer of last 1000 governance ops, replay from genesis if older. Storage cost analysis needed. |
| **U3** | **HLC semantics for `governance_position.applied_at`.** D2 time-window needs a concrete clock semantic. Is `applied_at` the peer-local wall-clock time of apply, or an HLC timestamp embedded in the governance op itself? Local time means each peer evaluates the window differently — leak surface. Op-included HLC means the admin's clock is authoritative — different leak surface. | D2 | Design call. Lean toward op-included HLC (consistent with the rest of the rule being a pure function of governance state). |
| **U4** | **TEE peer designation for bootstrap (E1's TEE branch).** Is `ReadOnlyTee` role sufficient, or do we need an explicit "trusted bootstrap source" flag? How is the TEE peer's hardware attestation surfaced to a joining peer that wants to verify it? | E1 (TEE part only — Owner part is independent and can ship first) | **Decision: defer the TEE branch entirely as a follow-up RFC.** E1 ships with Owner-only bootstrap in v1. TEE redundancy can be designed once attestation infra in the broader Calimero ecosystem stabilizes. |

### 4.2 Per-issue design questions — resolve during the issue's discovery phase

| # | Question | Issue |
|---|---|---|
| **U5** | `NamespaceStateHeartbeat` currently carries `{namespace_id, dag_heads}` (`crates/node/src/handlers/network_event.rs:189`). E2 needs to extend it with `state_hash`, `hlc`, signature. New broadcast variant or extend existing? | E2 |
| **U6** | Network deny-list scope: per-group, per-namespace, or global per-signer-identity? Same signer can be a member of group X (where they were removed) and group Y (where they're still active). | D1 |
| **U7** | Snapshot trigger and frequency. Periodic (every N ops, every T seconds) or Owner-triggered? Per-namespace configurable? | C5 |
| **U8** | Time-window W concrete default value. 24h working assumption; per-namespace configurable? | D2 |
| **U9** | K0 grace period length. Per-namespace configurable? | D3 |
| **U10** | Bootstrap unreachability default policy. Strict (block) or fall back to multi-source (E3)? Per-deployment configurable, but what's the default? | E1 / E3 |
| **U11** | `MemberRestored` semantics. Is the restored member assigned to a specific role (Member by default), or restored to whatever role they had before removal? Are post-cut writes that were buffered now applied, or do they remain rejected? | C3 |
| **U12** | B2 buffer eviction policy. Bounded buffer for buffered-on-unknown deltas creates a DoS surface (attacker floods with deltas referencing future governance state). Max size, eviction strategy, rate limit? | B2 |

### 4.3 Migration & rollout

| # | Question | Affects |
|---|---|---|
| **U13** | How are existing groups migrated when B1 lands? Pre-B1 deltas have no `governance_position`; post-B1 receivers must accept them or fail to bootstrap from groups created before the upgrade. | B1 rollout |
| **U14** | Backwards compat for `MemberRemoved` ops in the governance DAG that pre-date C1 (no embedded cut). C1 receivers need to handle "old-shape `MemberRemoved`" by deriving an implicit cut from the op's position. | C1 rollout |

---

## 5. Issues — ready for conversion to GitHub issues

### A. Observability & convergence detection

---

#### A1 — Expose `state_hash` on group info admin API

**Phase**: 1 · **Size**: S · **Depends on**: — · **Blocks**: A2, A3, E1, E2

**Summary**: Wire `compute_group_state_hash` through the admin API so callers can read the current group state hash. Mirrors `rootHash` on context info. Immediate value: e2e tests can poll for governance convergence instead of fixed-sleep.

**Scope**:
- Add `state_hash: [u8; 32]` to `GroupInfoApiResponseData` (`crates/server/primitives/src/admin/mod.rs:84`)
- Compute via `compute_group_state_hash` (`crates/context/src/group_store/meta.rs:75`) in the handler
- Document in admin API reference

**Acceptance criteria**:
- `GET /admin-api/groups/:group_id` returns `state_hash` as hex
- Two nodes that have converged on governance state return identical `state_hash`
- Two nodes that diverge (e.g. one missing a `MemberRemoved`) return different `state_hash`

**References**: §3.1 of the design doc, `crates/context/src/group_store/meta.rs:75`

---

#### A2 — `wait_for_governance_sync` workflow step in merobox

**Phase**: 1 · **Size**: M (release cascade) · **Depends on**: A1 · **Blocks**: e2e tests for any governance-related work

**Summary**: New merobox workflow step that polls `state_hash` across nodes and waits for convergence. Replaces fixed `wait, seconds: N` sleeps used today for governance ops in e2e tests.

**Scope**:
- Mirror `WaitForSyncStep` (which polls `rootHash`) for governance state hash
- Configurable timeout, poll interval, target node set
- Document in merobox workflow reference

**Acceptance criteria**:
- e2e test using `wait_for_governance_sync` waits exactly until all listed nodes converge on the same `state_hash`, not a fixed duration
- Test with intentional divergence: step times out cleanly without false success
- At least one existing e2e test (e.g. leave-context) migrated from `wait, seconds: N` to `wait_for_governance_sync`

**References**: A1, `merobox` repository

---

#### A3 — Hierarchical `NamespaceMerkle` for whole-subtree convergence

**Phase**: 4 · **Size**: L · **Depends on**: A1, B1, C1 · **Blocks**: —

**Summary**: Composite hash that covers `meta + members_root + governance_dag_root + snapshot_root + child_namespace_roots` for a namespace. Lets a peer detect whole-subtree convergence in one comparison instead of walking each context individually. Per design decision §2.12, this is the **only** place where we extract a reusable Merkle primitive — leaves are existing flat hashes (`compute_group_state_hash`, `Snapshot::root_hash`, governance_dag_root). No refactor of the leaf hashes; storage `Index<S>` stays untouched.

**Scope**:
- Extract a small `MerkleTree` primitive (algorithm only, no persistence): `from_leaves(&[[u8;32]]) → root`, optional `proof(idx)` / `verify(root, proof, leaf)` if needed by future consumers
- Define `NamespaceMerkle` composer that builds the tree from `[group_state_hash, governance_dag_root, snapshot_root, child_namespace_roots…]` for a given namespace
- Expose via admin API
- Update `NamespaceStateBeacon` (E2) to optionally carry it

**Acceptance criteria**:
- `MerkleTree` primitive is pure-function, no I/O, unit-tested
- `NamespaceMerkle` is deterministic across peers with the same governance + state
- Test: drift in a deeply nested context propagates to namespace root
- Test: drift in governance state propagates to namespace root
- API consumers can poll one hash to detect any subtree change
- **Non-goal:** refactoring `compute_group_state_hash` or `Snapshot::root_hash` to use the new primitive — they stay as-is.

**References**: §2.11–2.12 (design decisions), §3.1 (state hash), A1, E2

---

### B. Cross-DAG causal authorization

---

#### B1 — Add `governance_position` field to ContextDagDelta / CausalDelta

**Phase**: 2 · **Size**: M · **Depends on**: U1 resolved (wire-format strategy) · **Blocks**: B2, B3, C1, C2, C4, D2, A3

**Summary**: Replace the dead `governance_epoch: Vec<[u8; 32]>` field on state deltas with a `GovernancePosition` struct that carries the full cross-DAG reference (group_id, state_hash, governance DAG heads) at sign time. This is the foundational primitive that lets receivers enforce cross-DAG authorization.

**Scope**:
- Define `GovernancePosition { group_id: ContextGroupId, group_state_hash: [u8; 32], governance_dag_heads: Vec<[u8; 32]> }`
- Replace `governance_epoch` field on `ContextDagDelta` / `CausalDelta`
- Sender side: compute and embed accurate values (`crates/context/src/handlers/execute/mod.rs:731` is where the empty vec is populated today)
- Receiver side: deserialize and pass through to apply path (`crates/node/src/handlers/state_delta/mod.rs:33,88`)
- Migration: handle pre-B1 deltas in flight per U13 / U1

**Acceptance criteria**:
- New field present in wire format
- Senders embed accurate state_hash + governance DAG heads at sign time
- Receivers deserialize without errors
- Existing e2e tests pass
- Roundtrip serialization test
- Migration plan documented and reviewed

**Open questions**: U1 (wire-format strategy), U13 (migration)

**References**: §5.2 of design doc, `crates/node/src/handlers/state_delta/mod.rs:33`, `crates/context/src/handlers/execute/mod.rs:731`

---

#### B2 — Receiver-side buffering on unknown governance state

**Phase**: 2 · **Size**: M · **Depends on**: B1 · **Blocks**: B3

**Summary**: When a state delta arrives whose `governance_position` references governance DAG heads the receiver doesn't have yet, buffer the delta. Apply only after the local governance DAG has caught up and the referenced state hash matches. If the state hash mismatches after catch-up, reject (Byzantine sender or stale claim).

**Scope**:
- Add buffer keyed on unresolved governance head set
- On governance DAG advance, scan buffer for now-resolvable deltas
- Apply or reject based on state_hash comparison
- Bounded buffer (DoS surface — see U12)

**Acceptance criteria**:
- Unit test: delta with future governance_position is buffered, applied only after governance catch-up
- Replay-attack test: delta with stale state_hash is rejected after governance catches up
- Buffer-overflow test: bounded eviction works under flood
- e2e test: cross-partition delivery of a delta whose governance_position only resolves after partition heal

**Open questions**: U12 (buffer eviction)

**References**: §5.3 of design doc

---

#### B3 — Apply-time membership check via governance_position

**Phase**: 2 · **Size**: M · **Depends on**: B1, B2 · **Blocks**: C4, C6

**Summary**: Replace today's "is the signer currently a member?" with "was the signer a member at the governance state this delta references?" The check is a pure function of `(delta.signer, delta.governance_position, governance DAG)`. Subsumes `is_read_only_for_context` and extends membership check to `User`-storage actions (which today have no membership check at all).

**Scope**:
- Implement governance state history (ring buffer of state hash → membership snapshot — see U2)
- Replace `is_read_only_for_context` check at `crates/node/src/handlers/state_delta/mod.rs:96`
- Extend to `User`-storage actions in `crates/storage/src/interface.rs::apply_action` (lines 277–330)
- Keep `Shared`-storage causal writer-set check (#2266) — they compose

**Acceptance criteria**:
- Removed member's deltas with post-`MemberRemoved` governance_position are rejected
- Removed member's deltas with pre-removal governance_position are applied (forward-only)
- `User`-storage actions from non-members are rejected
- `Shared`-storage path still works per #2266
- e2e test: member removed mid-test, attempts to publish, deltas rejected on all peers post-removal

**Open questions**: U2 (history storage strategy)

**References**: §5.4 of design doc, `crates/context/src/group_store/namespace.rs:64` (current is_read_only_for_context), `crates/storage/src/interface.rs:260`

---

### C. Removal semantics & recovery

---

#### C1 — Admin-signed deterministic cut on `MemberRemoved`

**Phase**: 3 · **Size**: M · **Depends on**: B1, B3 · **Blocks**: C3, C4, C5, D3

**Summary**: Embed the cut position explicitly inside the signed `MemberRemoved` op (rather than letting it be implicit at each peer's apply-time). All peers evaluate B3's validity rule against the same embedded cut value, even if their governance DAG views differ.

**Scope**:
- Add `cut: GovernancePosition` field to `MemberRemovedOp`
- Admin computes cut at sign time
- B3 uses `MemberRemovedOp.cut` for the descend-from check, not `position_of(op)`
- Backwards compat for old-shape `MemberRemoved` in pre-existing governance DAGs (U14)

**Acceptance criteria**:
- New field present and signed
- Concurrent-partition test: peers on different governance DAG positions evaluate the same answer for a given delta
- Migration: old-shape `MemberRemoved` ops still honored (or upgrade-path documented)

**Open questions**: U14 (backwards compat)

**References**: §5.5 of design doc

---

#### C2 — Self-signed `MemberLeft` cut

**Phase**: 3 · **Size**: S · **Depends on**: B1 · **Blocks**: —

**Summary**: For voluntary departures, the leaver embeds their own `governance_position` at sign time. No admin gating. Forward-only applies the same way as for `MemberRemoved`.

**Scope**:
- Add `cut: GovernancePosition` field to `MemberLeftOp`
- Leaver computes at sign time
- B3 honors leaver-signed cut as authoritative for "X is no longer a writer"

**Acceptance criteria**:
- Self-leave test: leaver's pre-leave writes preserved on all peers
- Self-leave test: post-leave writes from same identity rejected
- Composes with existing `leave_context` / `leave_group` infra from #2280

**References**: §5.5 of design doc, [Membership & Leave architecture](../architecture/membership-and-leave.html)

---

#### C3 — Owner override / `MemberRestored`

**Phase**: 3 · **Size**: M · **Depends on**: C1 · **Blocks**: —

**Summary**: Owner can sign a `MemberRestored` op that reverses an admin's `MemberRemoved`. Recovery path for the case where a Byzantine or mistaken admin removed a legitimate member. Replaces what a quorum design would have offered.

**Scope**:
- Define `MemberRestoredOp { restored_member, prior_cut, role_to_restore }` signed by Owner
- Re-add the member to the group; reset the deny-list (D1) for that signer
- Re-evaluate buffered post-cut deltas: now valid post-restore?

**Acceptance criteria**:
- e2e test: admin removes X; Owner restores X; X's pre-removal state preserved; X can author new deltas after restoration
- Convergence test: peers that received different orderings of `MemberRemoved` and `MemberRestored` end up identical

**Open questions**: U11 (semantics of post-cut buffered deltas, role assignment)

**References**: §5.7 of design doc

---

#### C4 — Forward-only semantic: declare and enforce

**Phase**: 2 · **Size**: S · **Depends on**: B3, C1, C2 · **Blocks**: —

**Summary**: Document and enforce: pre-cut `governance_position` writes from a removed/left member are valid forever, regardless of arrival order. No retroactive invalidation.

**Scope**:
- Document in `docs/architecture/membership-and-leave.md`
- Add e2e tests for the partition-heal cases that exercise the rule

**Acceptance criteria**:
- e2e test: removed member's pre-removal writes that arrive via partition heal are applied identically on all peers
- Documented rule with rationale

**References**: §5.6 of design doc

---

#### C5 — Owner-signed snapshots for bounded rebuild

**Phase**: 3 · **Size**: L · **Depends on**: C1 · **Blocks**: C6

**Summary**: Periodic snapshots of group state, signed by Owner. Two uses: bounded rebuild from corruption, and bootstrap floor (deltas claiming pre-snapshot `governance_position` are rejected).

**Scope**:
- Snapshot data structure (state hash + governance DAG cut + Owner signature)
- Periodic / triggered creation (U7)
- Snapshot replay primitive
- Bootstrap floor enforcement in B3

**Acceptance criteria**:
- Owner can produce a valid signed snapshot
- Replay from snapshot + valid deltas reaches the same state as full genesis-replay
- Pre-snapshot-position deltas are rejected after the floor advances
- Test: forge snapshot fails signature verification

**Open questions**: U7 (frequency / trigger)

**References**: §7.1 of design doc

---

#### C6 — Local rebuild tool

**Phase**: 3 · **Size**: M · **Depends on**: C5, B3 · **Blocks**: —

**Summary**: Operator-invoked. Detects tainted state (deltas with rejected `governance_position`), rebuilds from most recent valid snapshot using only valid deltas.

**Scope**:
- CLI tool: `calimero rebuild --group <id>`
- Detection logic for tainted state
- Replay logic using valid deltas + snapshot

**Acceptance criteria**:
- Given a deliberately-corrupted store, tool rebuilds to convergent state with another (clean) peer
- Tool is idempotent (running twice produces same result)
- Reports detected taint sources

**References**: §7.2 of design doc

---

### D. DoS & Byzantine defenses

---

#### D1 — Network-layer deny-list keyed on signer identity

**Phase**: 1 · **Size**: S · **Depends on**: — · **Blocks**: —

**Summary**: **The load-bearing long-range defense.** On `MemberRemoved` apply, every peer adds the removed member's signer identity to a deny-list. Drops gossip messages whose inner Calimero signer matches — at the libp2p apply boundary, after decryption, before any further processing. Critical: keyed on **signer identity** (Calimero-layer key), not libp2p peer-id, otherwise the attacker rotates peer-id and bypasses.

**Scope**:
- Maintain deny-list per group (or per namespace, see U6) of removed signer identities
- Hook in the receive path where messages are decrypted and inner-signer is known, drop matched messages
- Persist deny-list across restarts
- Reset on `MemberRestored` (C3 dependency, but D1 ships first — needs a hook for later reset)

**Acceptance criteria**:
- Removed member's gossipsub messages dropped on every peer that has applied `MemberRemoved`
- e2e test: simulate delayed `MemberRemoved` apply on one peer; once applied, X's subsequent messages are dropped at that peer
- Deny-list survives node restart
- Bypass test: X rotates libp2p peer-id, messages still dropped (signer-id keying)

**Open questions**: U6 (scope: per-group / namespace / global)

**References**: §6.2 of design doc

---

#### D2 — Time-window validity on `governance_position`

**Phase**: 3 · **Size**: S · **Depends on**: B1 · **Blocks**: —

**Summary**: `delta.hlc < governance_position.applied_at + W`. Reject deltas claiming a position older than W. Belt-and-suspenders for D1; bounds the long-tail attack window if D1 has gaps.

**Scope**:
- HLC semantic for `applied_at` (U3)
- Reject path in B3
- Per-namespace configurable W (U8)

**Acceptance criteria**:
- Delta with HLC outside window is rejected
- Delta with HLC inside window is accepted
- W is configurable per-namespace

**Open questions**: U3 (HLC semantics), U8 (default W)

**References**: §6.3 of design doc

---

#### D3 — K0 deprecation after `MemberRemoved` + grace

**Phase**: 3 · **Size**: S · **Depends on**: C1 · **Blocks**: —

**Summary**: After `MemberRemoved` + grace period, drop K0-encrypted gossip at receive. Defends against malicious *active* members using K0 maliciously after rotation triggered by other events.

**Scope**:
- Track K0 deprecation timer per group
- After grace, drop K0-decrypted messages at receive
- Per-namespace configurable grace (U9)

**Acceptance criteria**:
- Pre-grace: K0 messages still accepted
- Post-grace: K0 messages dropped, K1+ accepted
- Test: replay K0-encrypted message after grace fails

**Open questions**: U9 (grace period default)

**References**: §6.3 of design doc

---

#### D4 — Per-peer rate limits at gossip apply layer

**Phase**: 4 · **Size**: M · **Depends on**: — · **Blocks**: —

**Summary**: Bandwidth / msg-rate caps per signer at gossip apply boundary. Generic gossipsub hygiene; only worth implementing if empirical evidence of slow-leak attacks emerges. **Defer until justified.**

**Scope**:
- Per-signer rate counter
- Configurable cap
- Drop above-cap messages with logging

**Acceptance criteria**: Deferred — write the issue when there's evidence the surface needs closing.

**References**: §6.3 of design doc

---

### E. Bootstrap & eclipse resistance

---

#### E1 — Owner peer-id-authenticated bootstrap endpoint

**Phase**: 4 · **Size**: M · **Depends on**: A1 · **Blocks**: E3, E4

**Summary**: Late joiner dials Owner by peer-id. libp2p handshake authenticates that the responder holds the corresponding private key — eclipse-resistant because a malicious peer cannot impersonate Owner. Owner returns: current member set, current state_hash, current governance DAG heads. From there the joiner fetches full state from any current member and verifies against the anchor's state_hash. **TEE-peer redundancy is deferred per U4 — Owner-only in v1.**

**Scope**:
- Define bootstrap request/response messages (member set, state hash, governance DAG heads)
- Owner's identity is part of `compute_group_state_hash` (already true per #2284) — joiner extracts from group genesis
- libp2p peer-id authentication is built in; multiaddr discovery via DHT or invite payload is fine because peer-id auth catches mismatch
- Configurable strict-vs-fallback policy with E3 (U10)

**Acceptance criteria**:
- Late joiner can bootstrap from Owner peer-id alone
- Eclipse-attack test: malicious peer cannot impersonate Owner via fake multiaddr (libp2p handshake fails)
- Stale-Owner attack test: malicious peer serves a doctored governance DAG; joiner's state hash mismatch against multi-source / beacon corroboration detected
- Strict-vs-fallback policy is configurable per deployment

**Open questions**: U10 (default policy)

**References**: §6.4 of design doc, U4 decision (TEE deferred)

---

#### E2 — Promote `NamespaceStateHeartbeat` to load-bearing alive-beacon

**Phase**: 1 · **Size**: M · **Depends on**: A1 · **Blocks**: —

**Summary**: Currently `NamespaceStateHeartbeat` carries `{namespace_id, dag_heads}` and is informational only (`crates/node/src/handlers/network_event.rs:189`). Extend with `state_hash` (and signature?) and use for bootstrap convergence detection — late joiner listens for a short window, sees what state hash multiple distinct members agree on, picks the most-advanced.

**Scope**:
- Extend message shape to `{namespace_id, group_id, state_hash, governance_dag_heads, hlc, signature}` (U5)
- Sign per-member
- Receiver: maintain short-window aggregation of beacons during bootstrap
- "Most-advanced" tiebreak: HLC dominance, deepest governance DAG

**Acceptance criteria**:
- Heartbeat carries state_hash and is signed
- Bootstrap window collects beacons from multiple peers, picks most-advanced
- Test: malicious beacon with forged signature is rejected
- Test: late joiner with multiple honest beacons converges on correct state hash, ignores stale beacon from a slow peer

**Open questions**: U5 (message shape — extend or new variant?)

**References**: §6.4 of design doc, `crates/node/src/handlers/network_event.rs:189`

---

#### E3 — Multi-source bootstrap fallback

**Phase**: 4 · **Size**: M · **Depends on**: E1 · **Blocks**: —

**Summary**: When Owner / TEE is unreachable, fall back to dialing ≥2 distinct peers and taking the longest valid governance DAG. Eclipse becomes a network-layer assumption (X must control all bootstrap candidates).

**Scope**:
- Configurable `bootstrap_min_sources` (default 2 or 3)
- Compare governance DAGs from multiple sources
- "Longest valid" = most ops, all signatures verified back to a common ancestor
- Configurable strict-vs-fallback policy (U10) — block bootstrap if Owner unreachable, or allow fallback

**Acceptance criteria**:
- Bootstrap completes when Owner offline if ≥N honest peers reachable
- Censoring source serving a strict prefix is detected when compared to a non-censoring source
- Configurable policy works as documented

**Open questions**: U10 (default policy)

**References**: §6.4 of design doc

---

#### E4 — Owner-transfer chain verification

**Phase**: 4 · **Size**: S · **Depends on**: E1 · **Blocks**: —

**Summary**: If Owner has been transferred since group genesis, late joiner must resolve to the current Owner via signed transfer chain. Each `TransferOwnership` is signed by the previous Owner; chain back to genesis is locally verifiable.

**Scope**:
- Joiner fetches governance DAG, walks `TransferOwnership` ops
- Verifies each transfer signed by the predecessor Owner
- Trusts the latest Owner the chain resolves to

**Acceptance criteria**:
- With N `TransferOwnership` ops, late joiner resolves to current Owner
- Tampered chain (bogus signature, missing predecessor) is rejected
- Confirms `TransferOwnership` exists in the role model (it does — `crates/context/src/group_store/namespace_governance.rs:751`)

**References**: §6.4 of design doc, `crates/context/src/group_store/namespace_governance.rs`

---

## 6. Out of scope (decided, do not relitigate)

- **Quorum / M-of-N voting / multisig-style approvals.** Off the table by design — liveness regression, breaks CRDT model, doesn't fit small groups, doesn't actually solve correctness. Resolution mechanisms must be deterministic-causal or role-hierarchical, not vote-based. Includes K-of-K admin signatures and similar "lighter" multi-actor variants.
- **Closing the long-range attack surface entirely.** Fundamentally undecidable in async consensus. The design bounds the surface (gossip-propagation time of `MemberRemoved` + partition duration), not eliminates it.
- **Token-economic incentives or BFT consensus on every governance op.** Calimero is not a public chain.
- **Full retroactive invalidation of removed-member CRDT contributions.** Forward-only is the realistic stance.
- **Hardware-backed identity beyond `ReadOnlyTee`.** Out of scope for this work.
- **Refactoring `compute_group_state_hash` to a Merkle-based hash.** Considered and rejected. Inclusion/exclusion proofs aren't load-bearing under Owner-anchored bootstrap (§2.11), and changing the hash function would invalidate every existing `SignedGroupOp.current_state_hash` reference (per #2284) — migration cost greatly exceeds the marginal value. Stays as flat SHA-256.
- **Light-client / proof-based verification deployments.** Bootstrap is anchored to Owner / TEE peers (§2.11). If this stops being the durable model, a follow-up RFC can revisit Merkle-leaf hashes.
- **Unifying storage `Index<S>` Merkle with the new `MerkleTree` primitive (A3).** Same algorithmic shape, different persistence layer; storage Merkle is hot-path and well-tested, refactor cost outweighs the gain. A future cleanup may unify them but is out of scope here.

---

## 7. Issue tracking suggestion

22 issues across 5 categories. Recommended GitHub structure:

- **One umbrella tracking issue** linking the four phases.
- **Per-category epic labels** (`area/observability`, `area/cross-dag-auth`, `area/removal-semantics`, `area/dos-defense`, `area/bootstrap`).
- **Phase milestones** (`phase-1-quick-wins`, `phase-2-foundational`, `phase-3-removal`, `phase-4-bootstrap-longtail`).
- **Each issue copy-pasted from §5** with title `[{category}{number}] {issue title}`.

The blocking unknowns in §4.1 should be resolved in either (a) the umbrella tracking issue's discussion, or (b) standalone design-decision issues filed before the dependent work issue.
