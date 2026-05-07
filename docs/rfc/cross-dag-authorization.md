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

**Architectural approach — reference-based coupling**

The state DAG (CRDT, high-frequency) depends on the governance DAG (signed log, low-frequency) for authorization. The two are integrated by **reference-based coupling**: each state delta carries a `governance_position` reference to the governance state it was authored against; receivers apply against the **referenced** state, not their current local state. This is the foundational architectural choice of the entire roadmap.

Why reference-based, not synchronous coupling, optimistic-with-rollback, or governance-as-CRDT:

- **Decoupled rates** — state writes don't wait on governance acks; CRDT throughput is unaffected.
- **Validity is a pure function of `(delta, canonical governance DAG)`** — same answer on every peer regardless of receive order or local clock. This is the property that prevents forks.
- **Buffer-on-unknown handles asynchrony cleanly** — when a state delta arrives ahead of its referenced governance state, buffer it. Don't apply optimistically (taint cascade) and don't reject (divergence). Wait until governance catches up, then decide.
- **Forward-only falls out naturally** — pre-cut writes stay valid forever; post-cut writes are rejected forever. No retroactive invalidation, no taint cascade.

The roadmap's foundational issues (B1, B2, B3, C1, C2, C4) are concrete primitives implementing this approach.

**Concrete primitives**

1. **`governance_position` on every state delta** (B1) — `{ group_id, group_state_hash, governance_dag_heads }` embedded in `ContextDagDelta` / `CausalDelta`, replacing the dead `governance_epoch`.
2. **Buffer-on-unknown** (B2) — receivers buffer state deltas whose `governance_position` references governance heads not yet observed locally. Decision deferred until governance catches up.
3. **Apply-time membership check via `governance_position`** (B3) — validity is a pure function of `(delta.signer, delta.governance_position, governance DAG)`. Subsumes today's partial `is_read_only_for_context` and extends to `User`-storage actions.
4. **Forward-only semantic** (C4, baked into B3) — writes from a pre-cut `governance_position` are valid forever, regardless of arrival order or partition path. **Core invariant** — without it, taint cascade returns.
5. **Cut declarations are per-actor signed, not vote-based.** Forced removal: admin embeds an explicit `cut: GovernancePosition` in the signed `MemberRemoved` op (C1). Voluntary leave: leaver embeds their own `governance_position` in `MemberLeft` (C2).
6. **Owner override is the recovery path** (C3). Owner can sign `MemberRestored` to reverse an admin's `MemberRemoved`. Replaces what a quorum design would have offered.

**Defenses & bootstrap**

7. **D1 (network-layer deny-list keyed on signer identity) is load-bearing.** Collapses the long-range attack surface to gossip-propagation time of `MemberRemoved`. **Per-group scope** — same signer identity can legitimately be a member of multiple groups. Implemented with a per-signer reverse index for lookup efficiency.
8. **D2 (time-window validity) is dropped, not deferred.** D1 covers the long-range surface; D2 was defense-in-depth against D1 implementation bugs. The right response to "what if D1 has bugs" is "test D1 properly," not "build a parallel defense system." Pre-1.0 break-freely lets us add D2 later if D1 has empirical gaps.
9. **D3 (K0 deprecation after `MemberRemoved` + grace) is independent of D1** — closes the encryption channel after grace. Earns its keep against malicious *active* members using K0 maliciously after rotation.
10. **Bootstrap pins to Owner peer.** Late joiner dials Owner by peer-id; libp2p handshake authenticates the responder holds the corresponding private key. **Strict default** — block bootstrap if Owner is unreachable. Per-deployment override flag (`bootstrap_fallback: bool`) opts into multi-source fallback (E3). Per-namespace policy is a v2 follow-up. **TEE redundancy is deferred entirely as a follow-up RFC.**
11. **`NamespaceStateBeacon` is a new broadcast variant**, not an extension of the existing `NamespaceStateHeartbeat`. Decouples high-frequency unsigned liveness pings from lower-frequency signed bootstrap-relevant beacons. Heartbeat stays cheap and informational; beacon is signed and load-bearing for E2 / bootstrap convergence detection.
12. **Snapshots are scoped to recovery, not long-range defense.** Owner-signed (single-sig). Bound rebuild scope; provide a bootstrap floor for stale-position rejection.

**Trust model & hashes**

13. **Owner is mandatory at group genesis.** Already enforced functionally — Owner immune to involuntary removal, cannot self-leave (must `TransferOwnership` first per `crates/context/src/group_store/mod.rs:1039`), included in `compute_group_state_hash`. C3, E1, E4 assume this without fallback.
14. **Owner-anchored bootstrap is the durable trust model — light-client / proof-based verification is not a target use case.** Late joiners trust Owner's signed answer (verified via libp2p peer-id auth); we do not need inclusion/exclusion proofs against group state. Drives several downstream decisions including keeping `compute_group_state_hash` as flat SHA-256 (no Merkle refactor).
15. **Layered hashes, not unified — reuse one Merkle primitive only where actually needed.** `compute_group_state_hash` (governance, flat SHA-256), `Snapshot::root_hash` (state, existing storage Merkle via `Index<S>`), and `governance_dag_root` (existing) stay independent and update at their own rates. `NamespaceMerkle` (A3) composes them hierarchically using a small reusable Merkle primitive that is **extracted on-demand when A3 lands, not speculatively**. Storage `Index<S>` is left untouched (hot path, well-tested, in-scope independently per #2238).

**Hard constraints**

16. **No quorum / M-of-N voting / multisig anywhere.** This is a hard constraint, not a preference. See §6 out of scope.
17. **Pre-1.0 backwards compatibility is not a constraint.** Wire-format / on-disk / API breaking changes ship at release boundaries. No versioned protocol envelopes, no migration shims, no dual-shape receivers.

## 3. Phases & sequencing

| Phase | Items | Goal |
|---|---|---|
| **1 — Quick wins** (parallel) | A1, A2, D1, E2 | Observability + immediate DoS surface reduction. No wire-format changes. |
| **2 — Foundational** (sequential) | B1 → B2 → B3 → C4 | The cross-DAG primitive. After this, validity is well-defined. |
| **3 — Removal flow + recovery** | C1, C2, C3, D3, C5, C6 (parallel where possible) | Deterministic cuts + Owner override + K0 deprecation + bounded rebuild. |
| **4 — Bootstrap & long-tail** | E1, E3, E4, A3, D4 | Eclipse-resistant join + whole-subtree convergence + rate limits. |

---

## 4. Remaining open questions

All blocking unknowns are resolved. What's left are per-issue design questions — small, can be settled during each issue's discovery phase, none of them gate starting implementation.

| # | Question | Issue |
|---|---|---|
| **U7** | Snapshot trigger and frequency. Periodic (every N ops, every T seconds) or Owner-triggered? Per-namespace configurable? | C5 |
| **U9** | K0 grace period length. Per-namespace configurable? | D3 |
| **U11** | `MemberRestored` semantics. Is the restored member assigned to a specific role (Member by default), or restored to whatever role they had before removal? Are post-cut writes that were buffered now applied, or do they remain rejected? | C3 |
| **U12** | B2 buffer eviction policy. Bounded buffer for buffered-on-unknown deltas creates a DoS surface (attacker floods with deltas referencing future governance state). Max size, eviction strategy, rate limit? | B2 |

### Decisions reached during review (for reference)

The following questions were considered and decided; they are recorded in §2 and need not be re-litigated:

- **U1 (wire-format strategy)** → hard cut at release boundary (no backwards compat per §2.17).
- **U2 (governance state history storage)** → full materialized history; revisit at C5 when snapshots provide a natural floor.
- **U3 (HLC for `applied_at`)** → moot; D2 dropped (§2.8).
- **U4 (TEE bootstrap branch)** → deferred entirely as a follow-up RFC.
- **U5 (heartbeat shape)** → new `NamespaceStateBeacon` variant, not extension.
- **U6 (deny-list scope)** → per-group with per-signer reverse index for lookup.
- **U8 (time-window W)** → moot; D2 dropped.
- **U10 (bootstrap default)** → strict, with `bootstrap_fallback: bool` per-deployment override; per-namespace policy is v2.
- **U13, U14 (migration / backwards compat)** → moot; pre-1.0 break-freely, no migration shims (§2.17).

---

## 5. Issues — ready for conversion to GitHub issues

### A. Observability & convergence detection

---

#### A1 — Expose `group_state_hash` on group info admin API + rename context's `root_hash`

**Phase**: 1 · **Size**: S · **Depends on**: — · **Blocks**: A2, A3, E1, E2

**Summary**: Wire `compute_group_state_hash` through the admin API so callers can read the current group's governance state hash. Mirrors the existing context-state-hash on context info responses. Immediate value: e2e tests can poll for governance convergence instead of fixed-sleep waits. Includes a small API-level rename for naming consistency.

**Naming convention**: the existing `root_hash` field on context responses is the **context state hash** (Merkle root over storage entries). The new field is the **group state hash** (governance state). To make the distinction unambiguous at the API surface:
- Existing: `ContextWithExecutors.root_hash` → renamed to `context_state_hash` (Rust + JSON, snake_case throughout)
- New: `GroupInfoApiResponseData.group_state_hash` (Rust + JSON, snake_case)
- **Internal storage primitive `Snapshot::root_hash`** stays as-is (it really is the Merkle root hash of the storage tree; that's the right name in storage terminology, and renaming would cascade across ~50+ call sites for cosmetic gain).

**Scope**:
- Add `group_state_hash: String` (hex-encoded) to `GroupInfoApiResponseData` (`crates/server/primitives/src/admin/mod.rs:84`)
- Compute via `compute_group_state_hash` (`crates/context/src/group_store/meta.rs:75`) in the handler
- Rename `root_hash` → `context_state_hash` on `ContextWithExecutors` and any other admin-API response struct that exposes the context state hash today
- Update merobox `WaitForSyncStep` to poll the renamed `context_state_hash` field (release cascade — paired update to merobox repo)
- Document in admin API reference

**Acceptance criteria**:
- `GET /admin-api/groups/:group_id` returns `group_state_hash` as hex string
- Two nodes that have converged on governance state return identical `group_state_hash`
- Two nodes that diverge (e.g. one missing a `MemberRemoved`) return different `group_state_hash`
- Existing context endpoints return `context_state_hash` instead of `root_hash`
- merobox e2e tests still work after the field rename
- No internal references to `Snapshot::root_hash` are touched (verify with a `git diff` audit)

**References**: §1 (problem — "no governance state hash on admin API"), `crates/context/src/group_store/meta.rs:75`, `crates/storage/src/snapshot.rs:35`

---

#### A2 — `wait_for_governance_sync` workflow step in merobox

**Phase**: 1 · **Size**: M (release cascade) · **Depends on**: A1 · **Blocks**: e2e tests for any governance-related work

**Summary**: New merobox workflow step that polls `state_hash` across nodes and waits for convergence. Replaces fixed `wait, seconds: N` sleeps used today for governance ops in e2e tests.

**Scope**:
- Mirror `WaitForSyncStep` (which polls the renamed `context_state_hash` after A1) for the governance equivalent — a new step polls `group_state_hash`
- Configurable timeout, poll interval, target node set
- Document in merobox workflow reference

**Acceptance criteria**:
- e2e test using `wait_for_governance_sync` waits exactly until all listed nodes converge on the same `group_state_hash`, not a fixed duration
- Test with intentional divergence: step times out cleanly without false success
- At least one existing e2e test (e.g. leave-context) migrated from `wait, seconds: N` to `wait_for_governance_sync`

**References**: A1, `merobox` repository

---

#### A3 — Hierarchical `NamespaceMerkle` for whole-subtree convergence

**Phase**: 4 · **Size**: L · **Depends on**: A1, B1, C1 · **Blocks**: —

**Summary**: Composite hash that covers `meta + members_root + governance_dag_root + snapshot_root + child_namespace_roots` for a namespace. Lets a peer detect whole-subtree convergence in one comparison instead of walking each context individually. Per §2 decision 15 (layered hashes), this is the **only** place where we extract a reusable Merkle primitive — leaves are existing flat hashes (`compute_group_state_hash`, `Snapshot::root_hash`, governance_dag_root). No refactor of the leaf hashes; storage `Index<S>` stays untouched.

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

**References**: §2 decisions 14–15, §3 (state hash), A1, E2

---

### B. Cross-DAG causal authorization

---

#### B1 — Add `governance_position` field to ContextDagDelta / CausalDelta

**Phase**: 2 · **Size**: M · **Depends on**: — · **Blocks**: B2, B3, C1, C2, C4, A3

**Summary**: Replace the dead `governance_epoch: Vec<[u8; 32]>` field on state deltas with a `GovernancePosition` struct that carries the full cross-DAG reference (group_id, state_hash, governance DAG heads) at sign time. This is the foundational primitive that lets receivers enforce cross-DAG authorization.

**Scope**:
- Define `GovernancePosition { group_id: ContextGroupId, group_state_hash: [u8; 32], governance_dag_heads: Vec<[u8; 32]> }`
- Replace `governance_epoch` field on `ContextDagDelta` / `CausalDelta` (hard cut, no backwards compat per §2.17)
- Sender side: compute and embed accurate values (`crates/context/src/handlers/execute/mod.rs:731` is where the empty vec is populated today)
- Receiver side: deserialize and pass through to apply path (`crates/node/src/handlers/state_delta/mod.rs:33,88`)

**Acceptance criteria**:
- New field present in wire format
- Senders embed accurate state_hash + governance DAG heads at sign time
- Receivers deserialize without errors
- Existing e2e tests pass
- Roundtrip serialization test

**Open questions**: none — U1 / U13 resolved (hard cut, no backwards compat).

**References**: §2 (architectural approach + decision 1), `crates/node/src/handlers/state_delta/mod.rs:33`, `crates/context/src/handlers/execute/mod.rs:731`

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

**Open questions**: none — U2 resolved (full materialized history; revisit at C5).

**References**: §2 (architectural approach + decisions 1–6), `crates/context/src/group_store/namespace.rs:64` (current is_read_only_for_context), `crates/storage/src/interface.rs:260`

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
- Hard cut, no backwards compat for old-shape `MemberRemoved` (per §2.17)

**Acceptance criteria**:
- New field present and signed
- Concurrent-partition test: peers on different governance DAG positions evaluate the same answer for a given delta

**Open questions**: none — U14 resolved (no backwards compat).

**References**: §2 decision 5 (cut declarations)

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

#### C4 — Forward-only semantic: enforce as core invariant

**Phase**: 2 · **Size**: M · **Depends on**: B3, C1, C2 · **Blocks**: —

**Summary**: Forward-only is a **core invariant** baked into B3, not just a documented rule. Pre-cut `governance_position` writes from a removed/left member are valid forever, regardless of arrival order. Without this property, taint cascade returns (§4.3 of the design doc). This issue is a test-coverage and architectural-lock-in pass: every code path that reaches the validity check must apply forward-only consistently, and the property must be regression-protected.

**Scope**:
- Audit every code path that reaches B3's apply-time check; ensure all use forward-only (no path retroactively invalidates pre-cut writes)
- Add property-style tests: for any sequence of (cut, delta) pairs where `delta.governance_position` predates the cut, all peers apply
- Add partition-heal e2e tests: removed member's pre-removal writes propagated through different paths converge identically on all peers
- Document in `docs/architecture/membership-and-leave.md` with the invariant called out as load-bearing
- Add invariant assertions / debug logs at the validity check site to make violations loud during development

**Acceptance criteria**:
- All code paths to the validity check use forward-only (audited and documented)
- Property tests cover the partition-heal / out-of-order arrival cases
- e2e test: removed member's pre-removal writes via partition heal are identical on all peers
- Regression suite includes a "what if someone added retroactive invalidation" canary test
- Architecture doc updated

**References**: §2 (architectural approach + decisions 1–6), §4.3 (taint cascade — the failure mode forward-only prevents)

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

**Open questions**: none — U6 resolved (per-group scope, per-signer reverse index for lookup).

**References**: §2.7 (load-bearing defense decision)

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
- **Strict default** (block bootstrap if Owner unreachable). Per-deployment `bootstrap_fallback: bool` config flag enables E3 multi-source fallback.

**Acceptance criteria**:
- Late joiner can bootstrap from Owner peer-id alone
- Eclipse-attack test: malicious peer cannot impersonate Owner via fake multiaddr (libp2p handshake fails)
- Stale-Owner attack test: malicious peer serves a doctored governance DAG; joiner's state hash mismatch against multi-source / beacon corroboration detected
- `bootstrap_fallback` flag default is `false` (strict)

**Open questions**: none — U10 resolved (strict default + override flag); U4 resolved (TEE deferred). Per-namespace policy is a v2 follow-up.

**References**: §2.10 (bootstrap decision), §2.13 (Owner mandatory)

---

#### E2 — Add `NamespaceStateBeacon` (signed, load-bearing for bootstrap)

**Phase**: 1 · **Size**: M · **Depends on**: A1 · **Blocks**: —

**Summary**: Introduce a **new** broadcast variant `NamespaceStateBeacon { namespace_id, group_id, state_hash, governance_dag_heads, hlc, signature }`. Per §2.11, this is a new variant rather than an extension of `NamespaceStateHeartbeat` — decouples high-frequency unsigned liveness pings (existing heartbeat) from lower-frequency signed bootstrap-relevant beacons (new). Late joiner listens for a short window during bootstrap and picks the state hash multiple members agree on.

**Scope**:
- Define `BroadcastMessage::NamespaceStateBeacon` variant alongside the existing `NamespaceStateHeartbeat`
- Sign per-member with the member's identity key
- Cadence: lower-frequency than heartbeat (e.g., once per minute or on significant state change) — TBD during implementation
- Receiver: maintain short-window aggregation of beacons during bootstrap
- "Most-advanced" tiebreak: HLC dominance, deepest governance DAG
- Existing `NamespaceStateHeartbeat` stays as-is (informational, unsigned, high-frequency)

**Acceptance criteria**:
- New variant defined; senders emit at configured cadence; signature verified on receive
- Bootstrap window collects beacons from multiple distinct members, picks most-advanced state hash
- Test: malicious beacon with forged signature is rejected
- Test: late joiner with multiple honest beacons converges on correct state hash, ignores stale beacon from a slow peer
- Test: existing `NamespaceStateHeartbeat` continues to fire at its previous cadence and is not affected

**Open questions**: none — U5 resolved (new variant).

**References**: §2.11 (heartbeat decision), `crates/node/src/handlers/network_event.rs:189`

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
- **Refactoring `compute_group_state_hash` to a Merkle-based hash.** Considered and rejected. Inclusion/exclusion proofs aren't load-bearing under Owner-anchored bootstrap (§2.14), and changing the hash function would invalidate every existing `SignedGroupOp.current_state_hash` reference (per #2284) — migration cost greatly exceeds the marginal value. Stays as flat SHA-256.
- **Light-client / proof-based verification deployments.** Bootstrap is anchored to Owner peers (§2.14). If this stops being the durable model, a follow-up RFC can revisit Merkle-leaf hashes.
- **Unifying storage `Index<S>` Merkle with the new `MerkleTree` primitive (A3).** Same algorithmic shape, different persistence layer; storage Merkle is hot-path and well-tested, refactor cost outweighs the gain. A future cleanup may unify them but is out of scope here.
- **D2 (time-window validity on `governance_position`).** Considered and dropped (§2.8). D1 is the load-bearing long-range defense; D2 was insurance against D1 implementation bugs, which are better addressed by testing D1 properly than by building a parallel defense. Pre-1.0 break-freely lets us add D2 later if D1 turns out to have empirical gaps with motivation. The HLC-on-governance-ops field that D2 would have required is also dropped — governance ops continue to use `nonce` + `parent_op_hashes` for ordering, not HLC.
- **TEE peer redundancy for bootstrap.** Deferred entirely. v1 ships with Owner-only bootstrap. TEE redundancy is a follow-up RFC once attestation infra in the broader Calimero ecosystem stabilizes.
- **Versioned protocol envelopes / migration shims for B1 / C1.** Pre-1.0 backwards compatibility is not a constraint (§2.17). Wire-format breaks ship at release boundaries.

---

## 7. Issue tracking suggestion

21 issues across 5 categories (was 22 — D2 dropped). Recommended GitHub structure:

- **One umbrella tracking issue** linking the four phases.
- **Per-category epic labels** (`area/observability`, `area/cross-dag-auth`, `area/removal-semantics`, `area/dos-defense`, `area/bootstrap`).
- **Phase milestones** (`phase-1-quick-wins`, `phase-2-foundational`, `phase-3-removal`, `phase-4-bootstrap-longtail`).
- **Each issue copy-pasted from §5** with title `[{category}{number}] {issue title}`.

All blocking design questions are resolved (see §4 "Decisions reached during review"). Per-issue questions in §4 can be settled during each issue's discovery phase.
