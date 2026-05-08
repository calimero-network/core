# Cross-DAG Authorization & Convergence — Implementation Roadmap

| | |
|---|---|
| **Status** | Roadmap — issue-ready |
| **Date** | 2026-05-08 |
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

**Encryption & key lifecycle**

7. **Encrypt by default; plaintext only for bootstrap.** Group-scoped ops (`NamespaceOp::Group`) are already encrypted with the relevant tree-level key. The unfinished symmetry is at namespace level: `RootOp` variants like `GroupCreated`, `GroupReparented`, `GroupDeleted`, `AdminChanged`, `PolicyUpdated` are currently plaintext on the namespace topic. **D5** moves them under a `NamespaceOp::EncryptedRoot` variant encrypted with the namespace key. Only `MemberJoined` and `KeyDelivery` stay plaintext (bootstrap: a joiner without the namespace key needs to read these). Result: anyone outside the namespace sees opaque ops on the topic.
8. **Every member exit rotates the subgroup key — no exceptions.** `MemberRemoved` already rotates for Restricted subgroups + namespace root, but currently *skips* Open subgroups (the §2256 "Option C" trade-off) and **`MemberLeft` doesn't rotate at all**. Both gaps close: `MemberLeft` triggers symmetric rotation (**S7**), and Open subgroups get their own per-subgroup key like Restricted ones (**S8**). The Open vs Restricted distinction reduces to **admission policy** (auto-admit any namespace member vs admin-invitation), not key distribution.
9. **D1 (network-layer deny-list keyed on signer identity) is DoS reduction, not correctness.** Once B3 lands as the apply-time authorization check, D1's role demotes from "load-bearing long-range defense" to "drop earlier in the pipeline so we don't burn CPU on verify_signature + decode for ops B3 will reject anyway." Still ships, but the urgency reduces and the scope tightens to governance ops only (state-delta side is covered by encryption + B3).
10. **D2 (time-window validity) is dropped, not deferred.** Covered above; included for completeness.
11. **D3 (K0 deprecation) demoted from "load-bearing" to "confidentiality hygiene."** Once B3 rejects post-removal writes regardless of decryption status, K0 deprecation grace is about ensuring removed members can't *read* old in-flight messages — not about authorization. Combined with decision #8 (every exit rotates), D3's role is small.

**Bootstrap & observability**

12. **Bootstrap pins to Owner peer (initial admission).** Late joiner dials Owner by peer-id; libp2p handshake authenticates the responder holds the corresponding private key. **Strict default** — block bootstrap if Owner is unreachable. Per-deployment override flag (`bootstrap_fallback: bool`) opts into multi-source fallback (E3). Per-namespace policy is a v2 follow-up. (TEE redundancy is in scope per decision #23; this decision is the Owner-only minimum.)
13. **`NamespaceStateBeacon` is a new broadcast variant**, not an extension of the existing `NamespaceStateHeartbeat`. Decouples high-frequency unsigned liveness pings from lower-frequency signed bootstrap-relevant beacons. Heartbeat stays cheap and informational; beacon is signed and load-bearing for E2 / bootstrap convergence detection.
14. **Snapshots scoped to recovery, not long-range defense (further demoted by #25).** Owner-signed (single-sig). After decision #25 (every removal carries `expected_state_hash_after_apply`), C5's role shrinks to "occasional refresh hashes between removals + bounded rebuild scope when local state is corrupted." Bootstrap-floor / fabrication-bound roles are subsumed by signed removal hashes.

**Trust model & hashes**

15. **Owner is mandatory at group genesis.** Already enforced functionally — Owner immune to involuntary removal, cannot self-leave (must `TransferOwnership` first per `crates/context/src/group_store/mod.rs:1039`), included in `compute_group_state_hash`. C3, E1, E4 assume this without fallback.
16. **Owner-anchored bootstrap is the durable trust model — light-client / proof-based verification is not a target use case.** Late joiners trust Owner's signed answer (verified via libp2p peer-id auth); we do not need inclusion/exclusion proofs against group state. Drives several downstream decisions including keeping `compute_group_state_hash` as flat SHA-256 (no Merkle refactor).
17. **Layered hashes, not unified — reuse one Merkle primitive only where actually needed.** `compute_group_state_hash` (governance, flat SHA-256), `Snapshot::root_hash` (state, existing storage Merkle via `Index<S>`), and `governance_dag_root` (existing) stay independent and update at their own rates. `NamespaceMerkle` (A3) composes them hierarchically using a small reusable Merkle primitive that is **extracted on-demand when A3 lands, not speculatively**. Storage `Index<S>` is left untouched (hot path, well-tested, in-scope independently per #2238).

**Hard constraints**

18. **No quorum / M-of-N voting / multisig anywhere.** This is a hard constraint, not a preference. See §6 out of scope.
19. **Pre-1.0 backwards compatibility is not a constraint.** Wire-format / on-disk / API breaking changes ship at release boundaries. No versioned protocol envelopes, no migration shims, no dual-shape receivers.

**Unification thesis**

20. **B3 is the only authorization rule.** Once B1+B2+B3+C4 ship, every "can this write apply?" question reduces to one function call: *"was `delta.signer` a member of `delta.group_id` at `delta.governance_position`?"* — same answer on every peer, deterministic, forward-only. Everything else (D1, D3, key rotation, key deprecation) is **defense-in-depth around this primitive**. The patchwork of partial checks (`is_read_only_for_context`, per-storage-type role checks, `SignedGroupOp.current_state_hash` divergence detection) collapses into B3 once the primitive is in place — see Phase 3 cleanup track (S1-S6).
21. **The post-unification invariant.** *"After your membership row is gone, you can read nothing further (key rotated on every exit, RootOps encrypted with namespace key) and write nothing further (B3 rejects post-cut writes regardless of how they're encrypted or who relays them)."* No Open-subgroup loophole, no `MemberLeft` rotation skip, no governance-op visibility leak.

**Owner/TEE-anchored sync**

22. **Sync requests target the trusted-anchor set.** Peers preferentially direct explicit sync requests (state-DAG catch-up, governance-DAG catch-up, full state replay) to peers in the trusted-anchor set: `{Owner} ∪ {Admins} ∪ {ReadOnlyTee members of the relevant group}`. The trust gate for `ReadOnlyTee` reuses the existing `TeeAdmissionPolicy` attestation chain (`crates/context/src/group_store/membership_policy.rs`) — no new role, no new flag. Plain `Member` and `ReadOnly` peers are NOT in the trusted set; they can serve sync requests if asked, but clients that target them accept that the resulting state may be inconsistent with canonical history and is not recoverable through protocol means. Gossipsub broadcast remains the real-time write path; B3 + D1 validate it independently.
23. **TEE redundancy is load-bearing, not deferred.** With the trusted-anchor set including `ReadOnlyTee` members, Owner becomes an availability convenience rather than a liveness requirement. Multiple TEE replicas can be placed close to user regions and balance sync load. Owner key compromise alone is not catastrophic if TEE attestations independently vouch for the same state. **Reverses the prior U4 deferral** — TEE is in scope for v1; the existing `TeeAdmissionPolicy` is the implementation gate, no new mechanism needed.
24. **Practical resolution of the long-range attack surface.** The historical "Byzantine X with old keys forges deltas, sneaks them in via sync from a malicious relay" attack collapses under decision #22. Forged deltas can only enter the namespace through (a) Owner serving them (Owner is honest by definition), (b) a TEE-attested peer serving them (attestation gates this), or (c) plain gossip (D1 + B3 reject). Bound on the long-range surface becomes "Owner OR all TEE attestations OR all honest peers' D1/B3 simultaneously compromised" — a much higher bar than "any malicious peer can amplify forgeries via sync."

**Anchor-attested canonical state on removal**

25. **`MemberRemovedOp` and `MemberLeftOp` carry `expected_state_hash_after_apply: [u8; 32]`** — admin (or leaver) signs the canonical state hash representing their local view *after* applying the removal. Receivers verify their own post-apply local state hashes to the same value. **Mismatch → reconcile.** This subsumes the C5 snapshot-sealing role: every removal op is an implicit signed snapshot. Replaces the earlier "state-DAG cut enumeration" idea with a single 32-byte hash — equivalent strength, dramatically simpler.
26. **Reconciliation strategy on hash mismatch — local replay OR sync from anchor.** If the receiver's post-apply state hash matches the signed value, no reconciliation needed. If it doesn't (the receiver applied X-deltas in flight that admin's view didn't include, or the receiver missed deltas admin had), the receiver chooses based on a local cost heuristic:
    - **Local replay** when the diff is small (recent snapshot exists, few intervening ops): walk the local state DAG, drop the X-authored deltas not in admin's view, replay forward, verify match.
    - **Sync from anchor** (Owner/TEE per #22) when replay would be expensive: request the canonical state, verify against the signed hash, adopt directly.
    - **Stay diverged** if no anchor reachable AND replay is infeasible: the mismatch is detectable; rebuild via C6 is the operator's recourse.

    Both reconciliation paths converge to the same `expected_state_hash_after_apply`. The decision is local and deterministic; two peers may pick differently and still reach the same state.

27. **Sync verification: defense-in-depth on cheap per-delta crypto, trust anchor for the expensive checks.** When a receiver adopts canonical state from an anchor (#26 heavy path), they re-verify the cheap, idempotent per-delta properties — Ed25519 signatures, nonce monotonicity per signer, HLC monotonicity per signer, User-storage entry signatures (Ed25519 against claimed owner), per-entry content hashes — then verify the final state hash matches the anchor's signed `expected_state_hash_after_apply`. The expensive checks (B3 membership-at-`governance_position`, ADR-0001 `writers_at(parents)` for Shared storage) are **trusted by virtue of the signed state hash** — anchor's signature attests they ran them correctly. Defense-in-depth: anchor compromise that propagates forged signatures or unsigned User entries is caught by the cheap checks and the final hash mismatch. Trust-root compromise (decision #13) is mitigated by multi-anchor agreement (decision #23 + E3), not by re-running B3 on every sync.

## 3. Phases & sequencing

Reorganized around the **unification thesis** (§2 decisions 20-21): the load-bearing work is the cross-DAG primitive; everything else is layered defense or cleanup of patchwork the primitive subsumes.

| Phase | Items | Goal | Status |
|---|---|---|---|
| **1 — Observability** | ~~A1~~, ~~A2~~, E2 | Convergence detection across nodes; foundation for E1 bootstrap. | A1+A2 done ([#2289](https://github.com/calimero-network/core/pull/2289), [merobox#223](https://github.com/calimero-network/merobox/pull/223)+[#224](https://github.com/calimero-network/merobox/pull/224)); E2 left. |
| **2 — The Unifier** (sequential) | ~~B1~~ → ~~B2~~ → ~~B3~~ → C4, plus C1, C2 | The cross-DAG primitive. After this, B3 is the only authorization rule. C1/C2 deterministic cuts make removals well-defined causally. | **B1+B2+B3 done** ([#2298](https://github.com/calimero-network/core/pull/2298)); C4 + C1 + C2 left. |
| **3 — Codebase cleanup** | S1–S12, ~~S13~~, S14–S16 | Collapse the patchwork B3 makes redundant; close key-lifecycle gaps; consolidate authorization-check duplication; decouple cryptography from visibility model; centralize writer-set resolution; type-shape refactors of surviving code (illegal-states-unrepresentable, newtype discipline, unified error contract). **Net code deletion + tighter types on what stays.** S15 was anticipated as a pre-requisite for B3 but the implementation didn't end up needing the `SignerId` newtype split — kept on the cleanup track for general type discipline. | **S13 done** as part of [#2298](https://github.com/calimero-network/core/pull/2298); S1–S12 + S14–S16 not started. |
| **4 — Removal flow & recovery** | C3, C5, C6 | Owner override, snapshots, rebuild tool. Built on the unifier. | not started |
| **5 — Encrypted-by-default & anchored sync** | D5, E1, E3, E4 | Encrypt RootOps with namespace key; eclipse-resistant join via Owner/TEE anchors; ongoing sync targets the trusted-anchor set. | not started |
| **6 — Defense-in-depth** | D1, D3, D4, A3 | Now-clearly-optional layers: network deny-list, K0 deprecation, rate limits, hierarchical Merkle. | not started |

### Side findings exposed by completed work

- **Subgroup state-hash divergence on join via invitation** ([#2292](https://github.com/calimero-network/core/pull/2292)) — pre-existing bug surfaced by A1's `groupStateHash` field. `join_group.rs:97-98` pre-populated `target_application_id = ZERO`, while inviters had the real value. Inheritance via `create_group_in_namespace` propagated the divergence to subgroups. Fixed by extending `SignedGroupOpenInvitation` with an unsigned `application_id` field. Lands with the e2e migration PR. *Underlying cleanup tracked as S1.*

- **B2 lazy-drain deadlock** ([#2298](https://github.com/calimero-network/core/pull/2298)) — the originally-shipped B2 drained the governance-pending buffer only on incoming state-delta receives. The 3-node E2E exposed the deadlock case: node-1 broadcasts a single state delta, node-2 buffers it for missing governance heads, no further state delta arrives to trigger drain, never converges. **Fix**: added an active-drain hook in the namespace-governance apply path so a governance op landing fires drain across all pending contexts. Lazy drain stays as the steady-state path; active drain handles the quiescent case.

- **Sender-side TOCTOU race in `compute_governance_position_for_context`** ([#2298](https://github.com/calimero-network/core/pull/2298)) — `compute_group_state_hash` and `read_head_record` are non-atomic relative to each other. A governance op landing between the two reads produces a position whose hash and heads describe different states. Worked around with a double-read pattern (read heads → compute hash → re-read heads → bail if heads changed). A true atomic fix requires refactoring those helpers to share a single store `Handle` for snapshot-consistent reads — *tracked as a focused follow-up*, not blocking #2298.

- **Wire-format DoS bound enforcement at three layers** ([#2298](https://github.com/calimero-network/core/pull/2298)) — `MAX_GOVERNANCE_DAG_HEADS = 32` enforced at: borsh `BorshDeserialize` (rejects pre-allocation), serde `deserialize_with` (rejects post-allocation for JSON), `GovernancePosition::new()` constructor (rejects local construction), and a runtime guard inside `membership_status_at`. The constructor + serde checks were added after PR review showed the borsh-only check left bypass paths via direct construction and JSON.

---

## 4. Remaining open questions

All blocking unknowns are resolved. What's left are per-issue design questions — small, can be settled during each issue's discovery phase, none of them gate starting implementation.

| # | Question | Issue |
|---|---|---|
| **U7** | Snapshot trigger and frequency. Periodic (every N ops, every T seconds) or Owner-triggered? Per-namespace configurable? | C5 |
| **U9** | K0 grace period length. Per-namespace configurable? | D3 |
| **U11** | `MemberRestored` semantics. Is the restored member assigned to a specific role (Member by default), or restored to whatever role they had before removal? Are post-cut writes that were buffered now applied, or do they remain rejected? | C3 |
| ~~**U12**~~ | ~~B2 buffer eviction policy.~~ **Resolved in [#2298](https://github.com/calimero-network/core/pull/2298)**: per-context cap = `MAX_GOVERNANCE_PENDING_PER_CONTEXT = 256` with FIFO eviction; per-delta `governance_drain_attempts: u8` with `MAX_GOVERNANCE_DRAIN_ATTEMPTS = 16`; duplicate-delta-id detection on push. Two drain triggers (lazy on state-delta receive + active on governance-op apply). | ~~B2~~ done |

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

#### ✅ A1 — Expose `group_state_hash` on group info admin API + rename context's `root_hash`

**Status**: **Done** — landed in [calimero-network/core#2289](https://github.com/calimero-network/core/pull/2289) (and paired client-py [#43](https://github.com/calimero-network/calimero-client-py/pull/43)/[#44](https://github.com/calimero-network/calimero-client-py/pull/44), merobox [#223](https://github.com/calimero-network/merobox/pull/223)/[#224](https://github.com/calimero-network/merobox/pull/224)/[#225](https://github.com/calimero-network/merobox/pull/225)). Calimero rc.35 ships the rename + new field.

**Phase**: 1 · **Size**: S · **Depends on**: — · **Blocks**: A2, A3, E1, E2

**Summary**: Wire `compute_group_state_hash` through the admin API so callers can read the current group's governance state hash. Mirrors the existing context-state-hash on context info responses. Immediate value: e2e tests can poll for governance convergence instead of fixed-sleep waits. Includes a small API-level rename for naming consistency.

**Naming convention** (consistent across the three levels of state hash exposed by the admin API):

| Level | Rust field | JSON field | Status |
|---|---|---|---|
| Context (storage Merkle root) | `context_state_hash` | `contextStateHash` | A1 (rename `root_hash`) |
| Group (governance flat hash) | `group_state_hash` | `groupStateHash` | A1 (new field) |
| Namespace (hierarchical Merkle) | `namespace_state_hash` | `namespaceStateHash` | A3, Phase 4 |

**JSON casing is camelCase**, not snake_case — that's the dominant convention in `crates/server/primitives/src/admin/mod.rs` (161 of 179 admin response structs use `#[serde(rename_all = "camelCase")]`). Rust struct fields stay snake_case as Rust idiom; serde renames at serialize time.

**Internal storage primitive `Snapshot::root_hash`** stays as-is (it really is the Merkle root hash of the storage tree — the right name in storage terminology — and renaming would cascade across ~50+ call sites for cosmetic gain).

**Scope**:
- Add `group_state_hash: String` (hex-encoded) to `GroupInfoApiResponseData` (`crates/server/primitives/src/admin/mod.rs:1679`)
- Compute via `compute_group_state_hash` (`crates/context/src/group_store/meta.rs:75`) in `crates/server/src/admin/handlers/groups/get_group_info.rs`
- Rename `root_hash` → `context_state_hash` on the public `Context` type (`crates/primitives/src/context.rs:99`) and on `ContextWithExecutors` (`crates/server/src/admin/handlers/context/get_contexts_with_executors_for_application.rs:17`)
- Add `#[serde(rename_all = "camelCase")]` to `ContextWithExecutors` to make it consistent with the rest of the admin API (it's currently the outlier serializing as snake_case)
- Update merobox `WaitForSyncStep` (`merobox/commands/bootstrap/steps/wait_for_sync.py`) to read `contextStateHash`; drop the `rootHash`/`root_hash` dual-fallback
- Update merobox `commands/context.py` "Root Hash" display path
- No internal `Snapshot::root_hash` / `meta.root_hash` / `context.root_hash` references touched

**Acceptance criteria**:
- `GET /admin-api/groups/:group_id` returns `groupStateHash` (camelCase JSON) as hex string
- `GET /admin-api/contexts/:id` returns `contextStateHash` instead of `rootHash`
- Two nodes that have converged on governance state return identical `groupStateHash`
- Two nodes that diverge (e.g. one missing a `MemberRemoved`) return different `groupStateHash`
- merobox e2e tests still pass after the field rename
- No internal references to `Snapshot::root_hash` / `meta.root_hash` / `context.root_hash` are touched (verify with `git diff`)

**Open questions**: none.

**References**: §1 (problem — "no governance state hash on admin API"), `crates/context/src/group_store/meta.rs:75`, `crates/storage/src/snapshot.rs:35`. Namespace-level `namespace_state_hash` deferred to A3.

---

#### ✅ A2 — Extend `wait_for_sync` to support governance convergence

**Status**: **Done** — landed in [calimero-network/merobox#223](https://github.com/calimero-network/merobox/pull/223) + [#224](https://github.com/calimero-network/merobox/pull/224) (extension + cleanup) + [#225](https://github.com/calimero-network/merobox/pull/225) (client-py 0.6.7 pin). First migration of an existing e2e workflow (group-leave-member) opened in [#2292](https://github.com/calimero-network/core/pull/2292), which also fixed a pre-existing subgroup state-hash divergence bug uncovered by the migration.

**Phase**: 1 · **Size**: M (release cascade) · **Depends on**: A1 · **Blocks**: e2e tests for any governance-related work

**Summary**: Extend the existing merobox `wait_for_sync` workflow step (rather than introducing a separate `wait_for_governance_sync`) so it can wait for state convergence (`contextStateHash`), governance convergence (`groupStateHash`), or both — depending on which IDs the test specifies. Single concept ("wait for things to be in sync"), one step type, optional fields. Replaces fixed `wait, seconds: N` sleeps used today for both state-only and governance-only e2e scenarios.

**Why unified instead of a separate step**: most governance-touching tests *also* care that state has converged (e.g. "removed X; verify their writes don't leak"). A unified step handles mixed scenarios in one poll loop with `max(state_time, governance_time)` instead of `state_time + governance_time` of two sequential steps. State-only and governance-only tests pay nothing extra — they just specify the relevant ID.

**Scope**:
- Make `context_id` optional on `wait_for_sync` (currently required)
- Add optional `group_id` parameter
- At least one of `context_id` / `group_id` must be specified (validation error otherwise)
- If `context_id` provided, poll `contextStateHash` per existing logic
- If `group_id` provided, poll `groupStateHash` via the group info endpoint (`GET /admin-api/groups/:group_id`)
- If both provided, poll both endpoints in parallel; success requires both to converge
- Update merobox workflow reference docs to describe the three usage patterns (state-only / governance-only / mixed)
- No new step type — same `wait_for_sync` keyword

**Acceptance criteria**:
- Existing tests that use `wait_for_sync: { context_id }` continue to work unchanged
- `wait_for_sync: { group_id }` waits exactly until all listed nodes converge on the same `groupStateHash`, not a fixed duration
- `wait_for_sync: { context_id, group_id }` waits for both to converge (success only when both match across nodes)
- Validation: `wait_for_sync` with neither id specified errors clearly
- Test with intentional governance divergence: step times out cleanly without false success
- At least one existing e2e test (e.g. leave-context) migrated from `wait, seconds: N` to `wait_for_sync` with `group_id`

**References**: A1, `merobox` repository, [merobox PR #223](https://github.com/calimero-network/merobox/pull/223) (initial paired update for A1; A2 extension lands in same PR)

---

#### A3 — Hierarchical `NamespaceMerkle` for whole-subtree convergence

**Phase**: 4 · **Size**: L · **Depends on**: A1, B1, C1 · **Blocks**: —

**Summary**: Composite hash that covers `meta + members_root + governance_dag_root + snapshot_root + child_namespace_roots` for a namespace. Lets a peer detect whole-subtree convergence in one comparison instead of walking each context individually. Per §2 decision 15 (layered hashes), this is the **only** place where we extract a reusable Merkle primitive — leaves are existing flat hashes (`compute_group_state_hash`, `Snapshot::root_hash`, governance_dag_root). No refactor of the leaf hashes; storage `Index<S>` stays untouched.

**Scope**:
- Extract a small `MerkleTree` primitive (algorithm only, no persistence): `from_leaves(&[[u8;32]]) → root`, optional `proof(idx)` / `verify(root, proof, leaf)` if needed by future consumers
- Define `compute_namespace_state_hash` (composer) that builds the tree from `[group_state_hash, governance_dag_root, snapshot_root, child_namespace_roots…]` for a given namespace
- Expose `namespace_state_hash: String` (Rust) / `namespaceStateHash` (camelCase JSON) on the namespace info admin API response (`NamespaceApiResponse` in `crates/server/primitives/src/admin/mod.rs`)
- Update `NamespaceStateBeacon` (E2) to optionally carry it

**Acceptance criteria**:
- `MerkleTree` primitive is pure-function, no I/O, unit-tested
- `namespace_state_hash` is deterministic across peers with the same governance + state subtree
- Test: drift in a deeply nested context propagates to namespace root
- Test: drift in governance state propagates to namespace root
- API consumers can poll one hash (`namespaceStateHash`) to detect any subtree change
- **Non-goal:** refactoring `compute_group_state_hash` or `Snapshot::root_hash` to use the new primitive — they stay as-is. The naming pattern `{level}_state_hash` is preserved across all three exposed hashes (context / group / namespace).

**References**: §2 decisions 14–15, §3 (state hash), A1, E2

---

### B. Cross-DAG causal authorization

---

#### B1 — Add `governance_position` field to ContextDagDelta / CausalDelta ✅ DONE

**Phase**: 2 · **Size**: M · **Depends on**: — · **Blocks**: B2, B3, C1, C2, C4, A3 · **Status**: landed in [#2298](https://github.com/calimero-network/core/pull/2298)

**As-shipped notes**:
* `GovernancePosition` lives in `calimero-context-config::types`. Wire format on `BroadcastMessage::StateDelta` is `Option<GovernancePosition>` — `None` for legacy non-group contexts, with the receiver verifying via `get_group_for_context` to prevent malicious bypass.
* Bounded by `MAX_GOVERNANCE_DAG_HEADS = 32` at three layers: manual `BorshDeserialize` (rejects pre-allocation), serde `deserialize_with` (rejects post-allocation for JSON), `GovernancePosition::new()` constructor (rejects local construction), and a runtime guard inside `membership_status_at` (defense-in-depth).
* Sender uses a double-read pattern in `compute_governance_position_for_context` to mitigate the read-heads / compute-hash / read-heads TOCTOU race — drops the position rather than ship inconsistent values.

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

#### B2 — Receiver-side buffering on unknown governance state ✅ DONE

**Phase**: 2 · **Size**: M · **Depends on**: B1 · **Blocks**: B3 · **Status**: landed in [#2298](https://github.com/calimero-network/core/pull/2298)

**As-shipped notes**:
* Per-context `governance_pending: VecDeque<BufferedDelta>` on `NodeState`, capped at `MAX_GOVERNANCE_PENDING_PER_CONTEXT = 256` with FIFO eviction.
* Two drain triggers: **lazy** (on every state-delta receive for the same context — cheap and self-clearing during steady-state traffic) and **active** (hooked into the namespace-governance apply path — fires when a governance op lands and unblocks pending state deltas). Without the active path, the lazy drain alone deadlocks when the only state delta in flight is one waiting for that very governance op (caught by the e2e 3-node test).
* Per-delta `governance_drain_attempts: u8` counter on `BufferedDelta`, dropped after `MAX_GOVERNANCE_DRAIN_ATTEMPTS = 16` to bound retry resource use.
* Pop-then-process pattern (one delta at a time) so a panic mid-iteration only loses the in-flight delta, not the rest of the queue.
* Duplicate-delta-id detection on push to handle gossipsub re-delivery cleanly.

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

**References**: §2 decisions 1-3 (architectural primitives)

---

#### B3 — Apply-time membership check via governance_position ✅ DONE (state-delta path)

**Phase**: 2 · **Size**: M · **Depends on**: B1, B2 · **Blocks**: C4, C6 · **Status**: state-delta path landed in [#2298](https://github.com/calimero-network/core/pull/2298); `User`-storage `apply_action` extension still pending.

**As-shipped notes**:
* `handle_state_delta` calls `membership_status_at(author_id, governance_position)` and matches on the four-variant result: `Member` → continue to apply path; `Removed { last_role }` / `NeverMember` → reject + warn (D1 hint included for `Removed`); `Unknown { needed }` → buffer (B2); `Err` (hash mismatch / corruption) → reject + warn.
* `is_read_only_for_context` check kept in two places: `handle_state_delta` (fast-path rejection before drain + B3) and `apply_authorized_state_delta` (defense-in-depth covering the governance-pending drain path that doesn't go through the entry handler).
* Anti-bypass: when `governance_position == None`, the receiver verifies the context is genuinely non-group via `get_group_for_context` — a group context with missing position is rejected as a likely bypass attempt.
* The `User`-storage extension at `crates/storage/src/interface.rs::apply_action` is **not yet** done and remains in scope for a follow-up. Today only the state-delta receive path enforces.

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

#### C1 — Admin-signed deterministic cut + canonical state hash on `MemberRemoved`

**Phase**: 3 · **Size**: M · **Depends on**: B1, B3 · **Blocks**: C3, C4, D3

**Summary**: Embed two signed claims inside `MemberRemovedOp`: (1) the **governance-DAG cut** so every peer evaluates B3's validity rule against the same value, and (2) the **expected post-apply state hash** so every peer can verify their local view converges with the admin's canonical view. Together these resolve the apply-lag race AND the late-arriving-delta divergence in one signed envelope. Replaces the earlier C1+C5 combo with a single, simpler primitive.

**Scope**:
```rust
pub struct MemberRemovedOp {
    pub group_id: ContextGroupId,
    pub removed_member: PublicKey,
    pub cut: GovernancePosition,                       // governance-DAG cut (B3 descend-from check)
    pub expected_state_hash_after_apply: [u8; 32],     // canonical state hash post-apply (decision #25)
    pub admin_signature: Signature,
}
```

- Admin computes both fields at sign time:
  - `cut` = governance-DAG position of this op
  - `expected_state_hash_after_apply` = `compute_state_root_hash(local_state)` after admin runs apply locally
- Receivers verify:
  - B3 uses `MemberRemovedOp.cut` for the descend-from check
  - After apply, compute local state hash, compare to signed value
  - On match: convergence achieved naturally
  - On mismatch: reconcile per decision #26 — local replay (cheap path) or sync from anchor (heavy path); both verify the result against the signed hash before adopting
- Hard cut, no backwards compat for old-shape `MemberRemoved` (per §2.19)

**Acceptance criteria**:
- New fields present and signed
- Concurrent-partition test: peers on different governance DAG positions evaluate the same answer for a given delta
- [T₂, T₃] divergence test: a peer that applied X-deltas during partition catches up, computes mismatched state hash, reconciles via anchor sync, reaches canonical state
- Sync verification test: cheap per-delta crypto checks (sigs, nonces, HLCs, User-storage entry sigs) run on synced data; final hash compared (decision #27)
- Tampered-anchor test: synced data with a forged signature → caught by per-delta verification → sync rejected, no state adopted

**Open questions**: none — U14 resolved (no backwards compat).

**References**: §2 decisions 5, 25, 26, 27

---

#### C2 — Self-signed `MemberLeft` cut + canonical state hash

**Phase**: 3 · **Size**: S · **Depends on**: B1 · **Blocks**: —

**Summary**: For voluntary departures, the leaver embeds their own `governance_position` AND `expected_state_hash_after_apply` at sign time, mirroring C1's structure. No admin gating; the leaver is honest by definition for self-leave (a Byzantine leaver claiming false canonical state hash is detected by other peers' hash verification — they'd reconcile against admin's canonical view via subsequent governance ops anyway).

**Scope**:
- Add both `cut: GovernancePosition` and `expected_state_hash_after_apply: [u8; 32]` to `MemberLeftOp`
- Leaver computes both at sign time
- Receivers verify their post-apply state hash; reconcile if mismatch (per decision #26)
- B3 honors leaver-signed cut as authoritative for "X is no longer a writer"
- S7 cleanup: route through symmetric rotation path so MemberLeft also rotates the key (same envelope mechanism as MemberRemoved)

**Acceptance criteria**:
- Self-leave test: leaver's pre-leave writes preserved on all peers (forward-only)
- Self-leave test: post-leave writes from same identity rejected
- State-hash convergence test: peers that received different sets of leaver's in-flight deltas converge to the signed canonical state via reconciliation
- Composes with existing `leave_context` / `leave_group` infra from #2280

**References**: §2 decisions 5, 25, 26, 27, [Membership & Leave architecture](../architecture/membership-and-leave.html)

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

**References**: §2 decision 6 (Owner override)

---

#### C4 — Forward-only semantic: enforce as core invariant

**Phase**: 2 · **Size**: M · **Depends on**: B3, C1, C2 · **Blocks**: —

**Summary**: Forward-only is a **core invariant** baked into B3, not just a documented rule. Pre-cut `governance_position` writes from a removed/left member are valid forever, regardless of arrival order. Without this property, taint cascade returns (the taint-cascade scenario (§3.3 of the original RFC, preserved in commit history)). This issue is a test-coverage and architectural-lock-in pass: every code path that reaches the validity check must apply forward-only consistently, and the property must be regression-protected.

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

**References**: §2 (architectural approach + decisions 1–6), taint-cascade scenario (the failure mode forward-only prevents)

---

#### C5 — Owner-signed periodic state-hash refreshes (significantly demoted)

**Phase**: 4 · **Size**: M · **Depends on**: C1 · **Blocks**: C6

**Summary**: Originally scoped as "periodic Owner-signed snapshots that double as a bootstrap floor + fabrication-bound for the long-range attack surface." After decision #25 (every removal op carries `expected_state_hash_after_apply`), most of that role is subsumed by C1/C2 — every removal IS a signed snapshot point. **C5 reframes as: occasional Owner-signed refresh hashes for the long stretches *between* removals**, so newly-bootstrapped peers and corrupted-state recoveries have a recent canonical hash to verify against. **No longer load-bearing for the §6.1 forgery surface** — that role moved to decisions #25-27.

**Scope**:
- Owner can sign `{group_id, governance_position, expected_state_hash, timestamp}` and gossip as a `RefreshHash` op
- Cadence is admin-triggered, not periodic by default (saves bandwidth on quiet groups; #U7 closed)
- Receivers verify their local state hash matches; reconcile via #26 if mismatch
- Used by C6 rebuild tool as a recovery anchor

**Acceptance criteria**:
- Owner can produce a valid signed snapshot
- Replay from snapshot + valid deltas reaches the same state as full genesis-replay
- Pre-snapshot-position deltas are rejected after the floor advances
- Test: forge snapshot fails signature verification

**Open questions**: U7 (frequency / trigger)

**References**: §2 decision 14 (recovery snapshots)

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

**References**: §2 decision 14 (recovery snapshots), C5

---

### D. DoS & Byzantine defenses

---

#### D1 — Network-layer deny-list keyed on signer identity *(narrow scope, DoS reduction)*

**Phase**: 6 · **Size**: S · **Depends on**: — (independent; can ship before B3) · **Blocks**: —

**Summary**: Drop gossip messages signed by removed members **at the receive boundary**, before `verify_signature` + decode + actor routing. **Narrow scope: governance ops only** (state-delta side is covered by encryption + B3 + every-exit rotation per decision #8). Once B3 is the load-bearing authorization rule, D1's role is DoS reduction — drop earlier in the pipeline so peers don't burn CPU on ops B3 will reject anyway. Also closes the pre-B3 window where forged governance ops could flow through to apply.

**Scope**:
- Per-group deny-list of removed signer identities (per-signer reverse index for lookup efficiency)
- Hook on `OpEvent::MemberRemoved` and `OpEvent::MemberLeft` to add the signer
- Drop check in `crates/node/src/handlers/network_event/namespace.rs` — between `verify_signature()` and `apply_signed_namespace_op()`
- State-delta path NOT touched (encryption + B3 cover that side)
- Persist deny-list across restarts
- Reset hook for future C3 (`MemberRestored`)

**Acceptance criteria**:
- Removed member's signed governance ops dropped on every peer that has applied `MemberRemoved` / `MemberLeft`
- e2e test: removed member tries to publish a governance op post-removal — receivers drop, no apply
- Deny-list survives node restart
- Bypass test: removed member rotates libp2p peer-id, messages still dropped (signer-id keying)

**Open questions**: none.

**References**: §2 decisions 8, 9, 20

---

#### D3 — K0 deprecation after exit + grace *(confidentiality hygiene)*

**Phase**: 6 · **Size**: S · **Depends on**: C1 · **Blocks**: —

**Summary**: After member exit (`MemberRemoved` / `MemberLeft`) + grace period, drop K0-encrypted messages at receive. Reframed post-unification: B3 already rejects post-exit *writes* regardless of decryption status, and decision #8 (every exit rotates the subgroup key) means a removed member's K0 is no longer the current key for new traffic anyway. D3's role is **confidentiality hygiene** — ensure removed members can't *read* old in-flight K0 messages once grace expires.

**Scope**:
- Track K0 deprecation timer per group, started at `MemberRemoved`/`MemberLeft` apply
- After grace, drop attempts to decrypt with K0 at receive (treat as if key not in keyring)
- Per-namespace configurable grace (U9)

**Acceptance criteria**:
- Pre-grace: K0 still in keyring (in-flight messages decrypt)
- Post-grace: K0 attempts fail; K1+ accepted
- Test: replay K0-encrypted message after grace fails to decrypt

**Open questions**: U9 (grace period default)

**References**: §2 decisions 8, 11

---

#### D4 — Per-peer rate limits at gossip apply layer

**Phase**: 4 · **Size**: M · **Depends on**: — · **Blocks**: —

**Summary**: Bandwidth / msg-rate caps per signer at gossip apply boundary. Generic gossipsub hygiene; only worth implementing if empirical evidence of slow-leak attacks emerges. **Defer until justified.**

**Scope**:
- Per-signer rate counter
- Configurable cap
- Drop above-cap messages with logging

**Acceptance criteria**: Deferred — write the issue when there's evidence the surface needs closing.

**References**: §2 decisions 9, 11 (D1/D3 demoted to defense-in-depth)

---

#### D5 — Encrypt RootOps with namespace key (except bootstrap)

**Phase**: 5 · **Size**: M · **Depends on**: namespace key lifecycle settled · **Blocks**: —

**Summary**: Today, `NamespaceOp::Group { ..., encrypted, ... }` encrypts group-scoped ops with the group key. But `NamespaceOp::Root(RootOp::*)` variants — `GroupCreated`, `GroupReparented`, `GroupDeleted`, `AdminChanged`, `PolicyUpdated` — are **plaintext** on the namespace gossipsub topic. D5 introduces `NamespaceOp::EncryptedRoot { encrypted, key_id }` and moves all these variants under it, encrypted with the namespace key. Only `RootOp::MemberJoined` and `RootOp::KeyDelivery` stay plaintext (a bootstrapping joiner has neither key yet). After D5, anyone outside the namespace sees opaque ops on the topic — privacy gain + structural symmetry with `Group` ops.

**Scope**:
- Add `NamespaceOp::EncryptedRoot { group_id: namespace_id, key_id, encrypted: EncryptedRootOp }` variant, mirroring the existing `Group` variant
- Move encryptable RootOps (`GroupCreated`, `GroupReparented`, `GroupDeleted`, `AdminChanged`, `PolicyUpdated`) under it
- Keep `RootOp::MemberJoined` + `RootOp::KeyDelivery` as plaintext (bootstrap)
- Define namespace-key lifecycle: stable until explicit rotation (not coupled to per-subgroup rotations from member exits)
- Receivers without the namespace key see opaque blobs and route only the bootstrap variants

**Acceptance criteria**:
- Plaintext variants: only `MemberJoined` + `KeyDelivery`
- All other RootOps encrypted with namespace key on the wire
- Bootstrap flow still works (joiner sees `MemberJoined` plaintext, receives `KeyDelivery`, then can decrypt `EncryptedRoot` going forward)
- Network topic eavesdroppers can no longer learn membership / role / policy / subgroup-tree changes
- Test: peer outside the namespace observing the topic gets only opaque RootOp blobs + plaintext bootstrap

**Open questions**: when does the namespace key rotate? Decision: stable, only rotated on explicit admin action (separate from per-subgroup-exit rotation). Cascading rotation defeats the symmetry purpose.

**References**: §2 decision 7, §2 decisions 20-21 (unification thesis)

---

### S. Codebase cleanup (enabled by B3)

These issues *delete code* that becomes redundant once B3 is the load-bearing authorization rule. Net negative LOC. Most are gated on Phase 2 shipping; S1 can land independently.

---

#### S1 — Unify `create_group` ↔ `execute_group_created` meta-write paths

**Phase**: 3 · **Size**: M · **Depends on**: — · **Blocks**: —

**Summary**: Today, `create_group.rs` (originator) pre-populates `GroupMetaValue` *before* publishing the op, and `execute_group_created` (apply path) checks `meta_existed` and skips the meta write to avoid clobbering the pre-populate. Two write paths produce **different values** for the same fields (e.g., `app_key`, `created_at`, `auto_join`), and the two paths gradually drift over time — this is the root cause of the [#2292](https://github.com/calimero-network/core/pull/2292) divergence bug. Make originators go through the same apply path remote peers use; one source of truth.

**Scope**:
- Remove the meta pre-populate at `create_group.rs:108-119`
- `execute_group_created` always writes meta (drop the `meta_existed` skip)
- Originator publishes the op, then applies it locally via the same apply path everyone else uses
- Drop the `meta_existed` divergence comment block at `namespace_governance.rs:731-745`

**Acceptance criteria**:
- Originator's `GroupMetaValue` for a freshly-created group is byte-identical to a remote peer's
- `compute_group_state_hash` matches across all peers from the moment of group creation, not just after the first context registration
- The `application_id`-in-invitation hack from #2292 becomes unnecessary (kept for backwards compat or also removed)

**References**: §2 decision 21, [#2292](https://github.com/calimero-network/core/pull/2292)

---

#### S2 — Remove `is_read_only_for_context` (subsumed by B3)

**Phase**: 3 · **Size**: S · **Depends on**: B3 · **Blocks**: —

**Summary**: B3 is the apply-time membership check on every state delta. The partial check `is_read_only_for_context` (`crates/context/src/group_store/namespace.rs:64`) becomes redundant. Delete the function, its callers, and the call site at `crates/node/src/handlers/state_delta/mod.rs:96`.

**Acceptance criteria**: function deleted; B3 covers all paths; no test regression.

**References**: §2 decision 20

---

#### S3 — Drop the `governance_epoch` dead field

**Phase**: 3 · **Size**: S · **Depends on**: B1 · **Blocks**: —

**Summary**: B1 introduces `governance_position`; the existing `governance_epoch: Vec<[u8;32]>` field on `ContextDagDelta` / `CausalDelta` is dead since #2237 Phase 11.2 (sent as `vec![]`, ignored on receive) and B1 obsoletes it entirely. Remove the field and its sender/receiver references.

**Acceptance criteria**: field gone from wire format and all serialization sites; no compile errors; no behavior change.

**References**: §2 decision 1

---

#### S4 — Collapse per-storage-type role checks in `apply_action`

**Phase**: 3 · **Size**: M · **Depends on**: B3 · **Blocks**: —

**Summary**: `crates/storage/src/interface.rs::apply_action` has separate role-check paths for `User` (Ed25519 + nonce only — no membership check) and `Shared` (causal `writers_at(parents)` per ADR-0001). After B3, both go through the same apply-time membership check at the receive boundary. The per-storage-type duplication can collapse into "B3 first, then storage-type-specific causal/nonce check."

**Acceptance criteria**: single membership-check primitive used by both storage types; ADR-0001 causal write-set logic preserved for `Shared`; nonce monotonicity preserved for `User`.

**References**: §3.4 (per-action verification), §2 decision 20

---

#### S5 — Remove `SignedGroupOp.current_state_hash` divergence check

**Phase**: 3 · **Size**: S · **Depends on**: B1, B3 · **Blocks**: —

**Summary**: `SignedGroupOp.current_state_hash` (per #2284) embeds the group state hash at sign time and rejects governance ops signed against a stale state. With B1's `governance_position` carrying the same information causally, this field becomes redundant — the receiver's check against the canonical governance DAG subsumes the per-op check. Remove the field and the divergence-detection logic.

**Acceptance criteria**: field gone from `SignableGroupOp` / `SignedGroupOp`; governance DAG causal validity (B3) is the only divergence-detection mechanism; no behavior change for honest peers.

**References**: §3.5 (governance_epoch field — same neutering pattern), §2 decision 20

---

#### S6 — Unify Open / Restricted subgroup authorization paths

**Phase**: 3 · **Size**: M · **Depends on**: B3, S8 · **Blocks**: —

**Summary**: Today, Open and Restricted subgroups have different authorization flows: Open subgroups encrypt with the namespace key (any namespace member can read without explicit join); Restricted subgroups have their own key with explicit `KeyDelivery`. After S8 gives every subgroup its own key, the only remaining difference is **admission policy**: Open auto-admits any namespace member (auto-issues `KeyDelivery`), Restricted requires admin invitation. Collapse the auth code paths to one; admission policy becomes a per-group flag, not a separate code path.

**Acceptance criteria**: single key-distribution code path covers both Open and Restricted; admission policy is a flag, not a branch in the auth check; existing Open semantics (any namespace member can join) preserved.

**References**: §2 decision 8, S8

---

#### S7 — `MemberLeft` triggers symmetric key rotation

**Phase**: 3 · **Size**: S · **Depends on**: — · **Blocks**: —

**Summary**: Today, `MemberRemoved` rotates the group key (atomic with the op publication via `sign_apply_and_publish_removal`), but `MemberLeft` uses the generic `sign_apply_and_publish` path with `removed_member: None` — no rotation. The leaver keeps K0 forever, until D3 grace if D3 ships. Make `MemberLeft` symmetric: route through `sign_apply_and_publish_removal` (or add an equivalent variant) so it also triggers rotation.

**Scope**:
- Refactor `leave_group.rs` and `leave_namespace.rs` to use the removal-publishing path
- Single function for both `MemberRemoved` and `MemberLeft` — both emit a key rotation
- Update the `removed_member: Option<&PublicKey>` argument or rename it to `exiting_member`

**Acceptance criteria**:
- After self-leave, the leaver's K0 is rotated to K1 on remaining members
- Leaver does not get a new envelope (excluded from `build_key_rotation` as expected)
- Test: leaver's post-leave K0-encrypted writes don't land at remaining members once K0 is dropped via D3

**References**: §2 decision 8

---

#### S8 — Per-subgroup key for Open subgroups (drop the namespace-key shortcut)

**Phase**: 3 · **Size**: M · **Depends on**: — · **Blocks**: S6

**Summary**: Today, Open subgroups skip key minting and encrypt with the namespace key. This is the §2256 "Option C trade-off" — it saves a `KeyDelivery` round-trip but means a subgroup-removed member retains read access via their namespace membership, and member-exit rotations are skipped entirely (decision #8 violation). S8 mints a per-subgroup key for every Open subgroup, making them symmetric with Restricted ones for crypto purposes. Open vs Restricted now differs only in admission policy (any namespace member can join Open vs admin-invite for Restricted).

**Scope**:
- On Open subgroup creation: mint a fresh per-subgroup key, store, distribute to admin (the creator) via in-place store write
- On Open subgroup join (auto-admission for namespace members): publish a `KeyDelivery` to the joiner the same way Restricted subgroups do
- Remove the "encrypt with namespace key for Open" branch in the publisher
- Member exit rotates the subgroup key (was previously skipped — decision #8)

**Acceptance criteria**:
- Every subgroup, Open or Restricted, has its own key in the keyring
- Removing a member from an Open subgroup rotates that subgroup's key (read access revoked, not just authorization)
- Open admission semantics preserved (any namespace member can still join without admin invite)
- Test: removed Open-subgroup member loses both write authorization (B3) and read access (key rotated)

**References**: §2 decision 8, S6

---

#### S9 — Consolidate `apply_action` verification path (single pre-apply pipeline)

**Phase**: 3 · **Size**: M · **Depends on**: B3, S2, S4 · **Blocks**: —

**Summary**: `crates/storage/src/interface.rs` has long-standing TODOs at lines 265, 503, 1856 about refactoring action verification into a separate function. Today verification is split: `apply_action` does pre-apply checks inline (signature, nonce), and `verify_action_update` (lines 1797-1862) re-opens metadata and re-runs checks. The duplication is per-storage-type (`User` vs `Shared`) and per-call-site (apply path vs verify path). After B3 + S2 + S4, every action should flow through one pipeline: B3 (auth gate) → storage-type-specific composables (User: nonce; Shared: causal writers_at). Eliminate the verify/apply split.

**Scope**:
- Extract a single `verify_action_pre_apply` function from the duplicated logic in `apply_action` and `verify_action_update`
- The function runs all pre-apply checks (signature, nonce, B3, storage-type-specific) in one pass
- Both apply path and any standalone verification call sites use it
- Resolves the line-265, line-503, line-1856 TODOs

**Acceptance criteria**: single verification pipeline; no duplicated nonce or signature logic across paths; storage-type branches collapse to small leaf functions called from the unified pipeline.

**References**: `crates/storage/src/interface.rs:265,503,1856,1797-1862`, §2 decision 20

---

#### S10 — Consolidate permission-check wrappers in `group_store`

**Phase**: 3 · **Size**: S · **Depends on**: B3 · **Blocks**: —

**Summary**: `crates/context/src/group_store/mod.rs` exposes three permission-check wrappers (`check_group_membership`, `require_group_admin`, `require_group_admin_or_capability` — lines 380, 397, 408), and `crates/context/src/group_store/namespace_membership.rs` (lines 38-64) has its own service-layer reimplementation of subset checks (inviter permission, role mapping). Two layers, partial overlap, no single gate. After B3 lands, the inviter / admin / membership checks all reduce to "look up role at governance_position." Collapse the wrappers into one function with role/capability parameters.

**Scope**:
- Define a single `check_authorization(store, group_id, signer, required_role_or_capability) -> Result<()>` function
- Replace the three wrappers + the service-layer reimplementation with calls to this function
- Remove the duplicated namespace_membership.rs subset

**Acceptance criteria**: one authorization-check function; all callers route through it; no role-check logic duplicated across files.

**References**: `crates/context/src/group_store/mod.rs:380,397,408`, `crates/context/src/group_store/namespace_membership.rs:38-64`, §2 decision 20

---

#### S11 — Decouple visibility (Open vs Restricted) from key-rotation publisher

**Phase**: 3 · **Size**: M · **Depends on**: S6, S8 · **Blocks**: —

**Summary**: `crates/context/src/group_store/group_governance_publisher.rs:200-229` has a special branch where the publisher inspects subgroup visibility (`encrypting_group_id == self.group_id`) and skips key rotation for Open subgroups (the §2256 "Option C" trade-off). This couples the cryptography layer to the visibility/admission model. After S6 + S8 (every subgroup has its own key, Open vs Restricted is admission-only), this branch becomes unnecessary — every removal rotates regardless of visibility. Remove the visibility-aware branch; the publisher's job becomes "always rotate on removal/leave," cleanly decoupled from admission semantics.

**Scope**:
- Remove the `if encrypting_group_id == self.group_id` branch at `group_governance_publisher.rs:217`
- Always rotate on removal/leave; visibility is consulted only for admission policy elsewhere
- Update `SubgroupVisibilitySet` op handling (lines 131-172) to be a pure metadata op, not coupled to encryption boundary

**Acceptance criteria**: publisher.rs has no visibility-aware branches; key rotation is uniform; visibility decisions live exclusively in admission/membership code paths.

**References**: `crates/context/src/group_store/group_governance_publisher.rs:131-172,200-229`, §2 decisions 7, 8

---

#### S12 — Centralize `effective_writers` resolution at sync-anchor layer

**Phase**: 3 · **Size**: M · **Depends on**: B3, decision #22 · **Blocks**: —

**Summary**: ADR-0001's `writers_at(parents)` causal writer-set computation is split across two layers today: `crates/node/src/sync/helpers.rs:91-120` pre-resolves writers via rotation-log lookup for DAG-based deltas, while `crates/storage/src/interface.rs:379-382` does a DAG-free fallback (stored_writers → action's claim). Two implementations of the same logical concept. After decision #22 (Owner/TEE-anchored sync), the anchor's signed `expected_state_hash_after_apply` (decision #25) attests that writer-set resolution was done correctly at sign time. Storage layer takes only the effective set, drops the fallback path.

**Scope**:
- Remove the DAG-free fallback at `crates/storage/src/interface.rs:379-382`
- Storage trusts pre-resolved `effective_writers` from the receive path
- Receive path centralizes the resolution; one implementation in `crates/node/src/sync/helpers.rs` (or moved to a shared crate)

**Acceptance criteria**: one writer-set resolution implementation; storage layer has no DAG-walking code; ADR-0001 logic lives in one place.

**References**: `crates/storage/src/interface.rs:379-382`, `crates/node/src/sync/helpers.rs:91-120`, ADR-0001, §2 decisions 22, 25

---

#### S13 — `MembershipStatus` enum (make illegal states unrepresentable) ✅ DONE

**Phase**: 2 (pre-requisite for B3) · **Size**: S · **Depends on**: — · **Blocks**: B3, S10 · **Status**: landed in [#2298](https://github.com/calimero-network/core/pull/2298) together with B1+B2+B3.

**As-shipped notes**:
* `MembershipStatus` enum has four variants: `Member(role)`, `Removed { last_role }`, `NeverMember`, `Unknown { needed: Vec<[u8; 32]> }`.
* `Unknown` carries the *full set* of missing governance heads (not just the first), so B2 buffers once and waits for all of them in parallel rather than O(n) sequential buffer-and-retry round-trips.
* `membership_status_at(store, signer, position)` has three branches:
  * **Fast path** — when local heads match `position.governance_dag_heads`, consult the materialized member set directly. Verifies `position.group_state_hash` against the locally-computed hash; mismatch → `Err` (tampering or local divergence). On this path the materialized set drops removed entries, so it conflates `Removed` into `NeverMember` — documented at the function level with instruction to callers to treat both as "not currently a member."
  * **Unknown** — any referenced head missing from local op log → `Unknown { needed }`.
  * **Prefix walk** — heads known but local DAG advanced past them. BFS through `parent_op_hashes` from `target_heads`, decrypts `NamespaceOp::Group` entries via local keyring (key selected by op `key_id`), replays the membership state machine for the signer, returns the full distinction. Bounded by `MAX_PREFIX_WALK_NODES = 10_000` (combined `visited + to_visit` ceiling) to cap resource use on adversarial DAGs.
* Tests: 14 unit tests on the resolver state machine (including 2000 random sequences via seeded xorshift64 + exhaustive 0–4-length sequences over the 4-element alphabet); 7 integration tests on `membership_status_at` against an in-memory `Store` covering all three branches + bypass paths.

**Original spec preserved below for context.**

**Summary**: `crates/context/src/group_store/mod.rs:748-780` and surrounding lookup paths express membership as `Option<GroupMemberRole>` — `None` silently means "not a member." Call sites that forget to add an explicit membership check pass through the `None` arm without flagging. B3's apply-time check needs richer information than this: *was this signer a member at governance_position X, and if not, are they removed (cut-position known) or never a member?* The bool/Option encoding cannot carry that distinction.

**Scope**:
- Introduce `enum MembershipStatus { Member(Role), Removed { at: GovernancePosition }, NeverMember }` in the membership module
- Replace `Option<GroupMemberRole>` returns at the membership-check boundary with `MembershipStatus`
- Call sites that previously fell through the `None` arm now must match all three variants — caught at compile time
- B3's authorization check consumes `MembershipStatus` directly

**Acceptance criteria**: no `Option<GroupMemberRole>` at the membership-check boundary; B3 takes `MembershipStatus` as input; "forgot to check membership" produces a non-exhaustive-match warning.

**References**: `crates/context/src/group_store/mod.rs:748-780`, §2 decisions 1, 2, 3 (B1/B2/B3), audit finding #5

---

#### S14 — `WriterContext` enum replacing `Option<BTreeSet<PublicKey>>`

**Phase**: 3 · **Size**: S · **Depends on**: — · **Blocks**: S12

**Summary**: `crates/storage/src/interface.rs:86-100` carries `effective_writers: Option<BTreeSet<PublicKey>>` on `ApplyContext`. Two valid states are encoded ambiguously: `Some(∅)` ("pre-resolved to empty — no one can write"), and `None` ("not pre-resolved — fall back to stored writers"). Caller must read comments to disambiguate intent. B3 will extend `ApplyContext` to carry `governance_position` (B1); piling another optional field onto an already-ambiguous one compounds the smell.

**Scope**:
- Replace `effective_writers: Option<BTreeSet<PublicKey>>` with `enum WriterContext { PreResolved(BTreeSet<PublicKey>), FallbackToStored }`
- Update construction sites (`crates/node/src/sync/helpers.rs:91-120` and storage callers)
- Match arms force callers to consider both cases explicitly

**Acceptance criteria**: no `Option<BTreeSet<PublicKey>>` field on `ApplyContext`; intent of "empty pre-resolved" vs "use stored" is encoded in the type, not in comments.

**References**: `crates/storage/src/interface.rs:86-100`, §2 decisions 22, 25, audit finding #6

---

#### S15 — `SignerId` newtype (newtype discipline at the auth boundary)

**Phase**: 2 (pre-requisite for B3) · **Size**: S · **Depends on**: — · **Blocks**: B3

**Summary**: Storage and group_store both pass `PublicKey` as a raw type for two distinct roles: "owner of a slot" and "signer of an op." When B3's apply-time membership check lands, signature verification (storage) and membership check (group_store) happen at different call sites — confusing one for the other today is a runtime bug that nothing prevents. Extracting a `SignerId` newtype distinct from owner/key types makes the auth boundary type-checked.

**Scope**:
- Define `pub struct SignerId(PublicKey)` in the membership module (or a shared types crate)
- B3's authorization function takes `SignerId`, not raw `PublicKey`
- Storage's `apply_action` takes `SignerId` for the writer claim; existing `owner: PublicKey` slot ownership stays as `PublicKey` (or extracts into `OwnerKey`)
- Conversions are explicit at module boundaries

**Acceptance criteria**: B3's signature is `fn check(signer: SignerId, group: GroupId, position: GovernancePosition) -> Result<()>`; mixing up signer and slot-owner becomes a compile error.

**References**: `crates/storage/src/interface.rs:277-328` (User arm), §2 decisions 1-3, audit finding #9

---

#### S16 — Unify error types at the handler boundary

**Phase**: 3 · **Size**: M · **Depends on**: S2, S3 · **Blocks**: —

**Summary**: Three error conventions collide today. `crates/context/src/handlers/*.rs` use ad-hoc `eyre::eyre!("...")` strings; `crates/context/src/group_store/mod.rs` returns `EyreResult` (an alias for `eyre::Result`); `crates/storage/src/interface.rs` returns typed `StorageError`. Library crates leak `eyre` upward instead of exposing a typed contract. After S2 (one delivery pipeline) and S3 (consolidate verification), a single error contract becomes both feasible and necessary — without it, the unified pipeline keeps one foot in stringly-typed errors.

**Scope**:
- Define `enum ContextOperationError` at the handler boundary, wrapping `StorageError`, `MembershipError`, `SignatureError`, `NetworkError`
- Library crates (`calimero-context`, `calimero-storage`) return their typed errors; only the handler/HTTP layer maps to `eyre::Result`
- Remove `eyre::eyre!("...")` ad-hoc constructions from library code
- `EyreResult` alias retired in library crates (kept only at the binary/HTTP boundary)

**Acceptance criteria**: no `eyre::eyre!` calls inside library crates; handler boundary has one typed error enum; error variants enumerate the failure modes that callers can branch on.

**References**: `crates/context/src/handlers/*`, `crates/context/src/group_store/mod.rs`, `crates/storage/src/interface.rs`, audit finding #10

---

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

**References**: §2 decisions 12, 13, 22, 23 (bootstrap + anchored sync)

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

**References**: §2 decisions 12, 13, 22, 23 (bootstrap + anchored sync), `crates/context/src/group_store/namespace_governance.rs`

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

38 issues across 6 categories (S category expanded to S1-S16 after the codebase audit + structural-engineering pass; D5 added; D2 dropped). Recommended GitHub structure:

- **One umbrella tracking issue** linking the six phases.
- **Per-category epic labels** (`area/observability`, `area/cross-dag-auth`, `area/removal-semantics`, `area/cleanup`, `area/dos-defense`, `area/bootstrap`).
- **Phase milestones** mapped to §3 phases.
- **Each issue copy-pasted from §5** with title `[{category}{number}] {issue title}`.

All blocking design questions are resolved (see §4 "Decisions reached during review"). Per-issue questions in §4 can be settled during each issue's discovery phase.

---

## 8. Threat model coverage

What the design protects against, organized by adversary type. The §2 unification thesis (decisions 20-21) and Owner/TEE-anchored sync (decisions 22-24) collapse most attacks to either *"blocked by B3"* or *"bounded by trust anchors."*

### 8.1 Active member with malicious intent

A member of the group whose role is currently authorized but who is acting in bad faith.

| Attack vector | Defense | Status |
|---|---|---|
| Forge ops they're not authorized for (Member acts as Admin) | Signature verification — only Admin signing key can sign Admin ops; signer's role checked at apply | ✅ Existing crypto + role check |
| Submit deltas with bogus `governance_position` | B3 verifies `state_hash` claim against canonical history; mismatch → reject | ✅ B3 |
| Submit valid-but-harmful state data (CRDT garbage, storage flood) | App-layer concern, not protocol authz — see §8.4 | ⚠️ App responsibility |
| Spam writes (rate-based) | D4 per-peer rate limits | ✅ D4 (deferred but designed) |
| Replay old ops to confuse state | State DAG dedup by `delta_id` (content hash); HLC + nonce monotonicity | ✅ Existing CRDT semantics |
| Frame other peers (claim someone else's signature) | Signature verification — can't forge another peer's signature without their privkey | ✅ Existing crypto |
| Cause divergence via partition timing | Forward-only validity rule — same answer on every peer regardless of arrival order | ✅ Forward-only / B3 |
| Withhold gossip / refuse to relay | Gossipsub mesh redundancy + scoring per libp2p | ✅ Existing libp2p |
| Spam invitations / membership ops | Role-gated apply path + D4 rate limits | ✅ Role check + D4 |
| Steal another peer's identity (key compromise) | Not protocol-level — see §8.4 | ⚠️ Key custody |

### 8.2 Removed member with malicious intent

A member whose membership row is gone (via `MemberRemoved` or `MemberLeft`) but who retains their old signing key K0 and acts in bad faith.

| Attack vector | Defense | Status |
|---|---|---|
| Continue publishing state deltas with old key | B3 rejects post-cut writes; D1 drops at network layer; key rotation makes K0 stale; D3 closes K0 read after grace | ✅ Layered |
| Continue publishing governance ops | Apply-time role check rejects (no longer admin); D1 drops at network layer; D5 makes RootOps unreadable to ex-members anyway | ✅ Layered |
| Read post-removal state via retained K0 | Decision #8 (every exit rotates the key) — K1 minted, ex-member excluded from envelopes; D5 encrypts RootOps; D3 K0 grace closes the in-flight window | ✅ Decision #8 + D5 + D3 |
| Forge "historical" pre-cut deltas via sync from a malicious relay | **Decision #22** (Owner/TEE-anchored sync) — peers don't sync from random members; trusted anchors don't have the forgeries to serve | ✅ Decision #22 |
| Sneak forged deltas via gossipsub | D1 drops at libp2p apply boundary on peers post-MemberRemoved; apply-lag window peers are bounded by gossip-propagation time; forward-only convergence prevents fork | ✅ D1 + forward-only |
| Eclipse a late-joining peer | E1 bootstrap pins to trusted-anchor set; libp2p peer-id auth prevents impersonation | ✅ E1 |
| Replay old governance ops | Governance DAG dedup by op hash; per-signer nonce monotonicity | ✅ Existing |
| Withhold acks to stall governance | Ack router has timeout; publisher proceeds anyway (best-effort delivery) | ✅ Existing |
| **Long-range attack:** retain K0, sign new deltas claiming pre-cut `governance_position`, smuggle in via apply-lag windows | Bounded by decisions #22, #8, #25, and D1 — **not eliminated, but reduced to small windows** | ⚠️ Bounded residual |
| DoS via volume | D1 drops removed-member gossip; D4 rate limits any peer | ✅ D1 + D4 |

### 8.3 Concrete scenarios — partition + removal

The combination of decisions #25 (signed canonical state hash on removal), #26 (replay-or-sync reconciliation), and #27 (sync verification model) resolves several otherwise-tricky scenarios. Walking through them explicitly because they're the load-bearing cases.

#### 8.3.1 The cryptographic split-brain when MemberRemoved partitions

Three nodes; admin publishes `MemberRemoved {X}` at governance position H_remove. The op carries (a) the encrypted `MemberRemoved` itself, (b) the per-current-member rotation envelopes for new key K1, and (c) signed `expected_state_hash_after_apply`.

Scenario: A and B receive the op and apply. C is partitioned and doesn't.

| | Has K1 | Has K0 | Applied MemberRemoved | Knows expected hash |
|---|---|---|---|---|
| A, B (post-apply) | ✓ | ✓ (until D3 grace) | ✓ | ✓ |
| X (removed) | ✗ (no envelope) | ✓ | ✗ | ✗ |
| C (partitioned) | ✗ | ✓ | ✗ | ✗ |

**Reads**: A↔B works (both have K1). A→C and B→C fail to decrypt at C (C lacks K1). C→A and C→B succeed during D3 grace (A/B still have K0 in keyring), fail after grace.

**Self-isolation property**: once D3 grace expires on A/B, C is fully cryptographically isolated. C's K0-encrypted writes can't be decrypted. C's reads of new K1-encrypted writes fail. The partitioned peer KNOWS something is wrong via decryption failures — clear signal to operators.

**Self-healing property**: when C eventually receives `MemberRemoved` (via gossipsub backfill, sync from anchor, or partition heal):
1. C extracts the rotation envelope addressed to C (it was wrapped at sign time, when C was still a member)
2. ECDH-unwraps to get K1
3. Stores K1 in keyring; K0 stays during C's own D3 grace
4. C can now decrypt all K1-encrypted traffic

Cascading rotations (e.g., another member removed during C's partition) chain causally — C applies them in DAG order, each rotation produces the next key. C eventually arrives at K_current.

#### 8.3.2 The [T₂, T₃] divergence: legitimate writes from X during partition

This is the harder scenario. Even with self-healing crypto:

- T₀: admin publishes MemberRemoved
- T₁ = T₀ + ε: A and B apply, rotate K0→K1
- T₂ = T₁ + D3_grace: A and B drop K0 from keyring
- T₃: C catches up

The window [T₂, T₃] is where forward-only validity and the encryption layer pull in opposite directions:

- **X publishes a state delta D in [T₂, T₃]** (legitimate or Byzantine — same observable behavior)
- **A and B**: K1 is current; K0 is dropped. D was K0-encrypted. Decryption fails. **Drop silently.**
- **C** (still on K0, hasn't applied MemberRemoved): D decrypts fine. B3 says forward-only valid (D's `governance_position` predates the cut). **Apply.**

Naively, C diverges from A and B: C applied D, A and B don't have it. After T₃ when C catches up, the divergence persists in C's local state.

**Resolution via #25-27**: when C applies `MemberRemoved` at T₃, it computes its own state hash and compares to the signed `expected_state_hash_after_apply`:

- C's state hash includes D's effects.
- Admin's signed hash does NOT (admin's view didn't include D when signing).
- **Mismatch detected.**

C then reconciles per decision #26:

- **Local replay** (cheap path) if C can identify the delta(s) admin's view didn't include — drop them, replay forward, verify hash matches.
- **Sync from anchor** (heavy path) if replay would be expensive — request canonical state from Owner/TEE, verify against signed hash, adopt directly.

Either way, C ends at the canonical post-apply state. **Convergence achieved.**

The trade-off explicit in this design: legitimate late-arriving pre-cut writes from X that didn't reach admin in time **are dropped from the canonical state**. Admin's view at sign time defines what's canonical. C's interim apply of D was reasonable, but ultimately overridden by admin's signed claim.

For deployments where this trade-off is unacceptable (e.g., partition-prone networks where many legitimate late-arrivals are expected), longer D3 grace + more anchor replicas reduces the [T₂, T₃] window. The design accepts that the trade-off can't be eliminated entirely — see §8.5 for the bounded residual.

#### 8.3.3 Sync verification — defense-in-depth on cheap crypto, trust anchor for B3

When C reconciles via "sync from anchor" (decision #26 heavy path), the anchor sends the canonical state tree. C must verify the sync is real before adopting.

**What C verifies (cheap, always)**:

| Check | Cost | Catches |
|---|---|---|
| Each delta's Ed25519 signature against claimed signer | ~µs per delta | Forged attribution; anchor-injected unsigned data |
| Per-delta nonce monotonicity per signer | O(1) per delta | Replay; out-of-order injection |
| Per-delta HLC monotonicity per signer | O(1) per delta | Non-causal ordering |
| Each User-storage entry's signature (Ed25519 against claimed owner) | ~µs per entry | User-storage forgery — User storage's authentication model IS this signature |
| Each entry's content hash matches claimed hash | O(content size) | In-transit tampering |
| Final state root hash matches signed `expected_state_hash_after_apply` | O(1) | Anchor-data inconsistency or any of the above slipping through |

**What C trusts the anchor for (expensive, signed)**:

| Check | Why trusted |
|---|---|
| B3 membership-at-`governance_position` for each delta | Re-verifying requires walking governance DAG history at every delta's claimed position — that's the entire replay cost. Anchor's signature on `expected_state_hash_after_apply` IS the attestation that they ran B3 correctly. |
| ADR-0001 `writers_at(parents)` causal writer-set checks for Shared storage | Same cost reasoning. |
| Forward-only correctness | Implicit in B3; covered by the same signed attestation. |

**Two attack classes to consider**:

- **Class 1: Compromised anchor sends bad data** (forged signature, unsigned User entry, tampered content). Caught by the cheap verifications. Even if the anchor's signed hash matches their bad data, the per-delta sig check fails locally → sync rejected, no state adopted.
- **Class 2: Compromised anchor pre-validates incorrectly** (anchor accepted a forgery during their own apply). Not caught by cheap verifications — the data is cryptographically valid, just shouldn't have been authorized. Mitigation: multi-anchor agreement (decision #23 + E3) — if multiple TEE-attested anchors independently agree on the same `expected_state_hash_after_apply`, the attack has to compromise all of them.

For single-anchor deployments, Class 2 is the trust-root assumption (decision #13). For multi-anchor deployments, it requires concurrent compromise across diverse hardware. Decision #23 makes this the design's hardening recommendation for production.

#### 8.3.4 Recovery from a state where reconciliation isn't possible

If C is partitioned long enough that:
- D3 grace expires on A/B (so X's K0-encrypted late writes can't even be decrypted on A/B)
- C applied many local-only deltas (large divergence)
- Anchor unreachable (E1/E3 fall back paths exhausted)

C is in the "we don't care if they're fucked" state per decision #22. Recovery is not protocol-automatic — operator-driven via the C6 rebuild tool:

1. Operator detects the divergence (state hash mismatch logged loudly)
2. Operator runs the rebuild tool (C6) — wipes local state, re-bootstraps from anchor, re-applies from a fresh canonical state
3. C is back in sync; local-only deltas are lost (acknowledged trade-off)

The protocol's role is to make divergence **detectable and recoverable**, not to auto-reconcile under all conditions. Manual operator intervention via C6 is the documented escape hatch.

### 8.4 What's NOT covered (and why — application-layer & trust-root concerns)

The threat surface above is what the **protocol authorization layer** addresses. Several other concerns sit outside this layer by design:

#### 8.4.1 Application-layer harmful data (valid-but-bad payloads)

**Out of scope for cross-DAG authz.** The protocol layer answers *"who can write?"* — applications answer *"what they can write."*

This is intentional and sound: protocol authz can't reason about whether a CRDT write is "good" or "bad" — that's domain knowledge living in the application. The right place to enforce data correctness is the **data structures themselves**, designed so invalid states are unrepresentable:

- **`Shared` storage type** (`crates/storage/src/`) already provides causally-aware multi-writer CRDT semantics with `writers_at(parents)` per [ADR-0001](../adr/0001-shared-storage-concurrent-rotation.md). The data structures themselves bound what kind of corruption is possible.
- **Counter-style ops** (`Counter`, `PNCounter`) — can't write arbitrary values; can only `inc(n)` / `dec(n)`. A malicious member can spam increments but can't fabricate "alice's balance is suddenly 10^18".
- **Grow-only sets** (`GSet`) — adds only; can't remove existing members. A malicious member can add garbage entries but can't censor others' adds.
- **LWW with HLC tiebreaking** — last-writer-wins on HLC ordering. Forces causal consistency; a malicious member can write but can't pretend their write came earlier than it did.
- **Schema-validated writes** at the WASM contract boundary — applications can reject malformed writes before they hit storage. This is no different from any database's CHECK constraints.
- **Append-only logs** (`OpLog`) — never overwrite; auditable history. Members can append but can't fabricate or censor history.

The discipline this asks of application developers: **design for the trust boundary**. If you don't trust members not to write garbage, don't give them a `Map<String, String>`. Give them a CRDT type whose invariants make "garbage" structurally impossible (or at least bounded).

What protocol authz **does** give the application:
- B3 guarantees the writer was authorized AT THE GOVERNANCE STATE the write claims
- Forward-only guarantees pre-cut writes from a former member are stable across peers
- Signed-by-member guarantees the write attribution is honest (the signer really is who claims to have written it)

What the application has to do:
- Choose a data model where authorized but malicious writes are bounded in what damage they can do
- Optionally enforce schema invariants in WASM at write-application time (the `apply_action` hook already supports rejecting malformed writes)

This is the same trade-off any blockchain or distributed-system makes: protocol authz is "who can write," application logic is "what they can write." Conflating them creates either a protocol that's too rigid for arbitrary apps, or an application logic that has to re-implement signature verification.

#### 8.4.2 End-user private key compromise

**Out of scope for protocol authz.** Calimero is signature-based; if an adversary obtains a member's private key, the adversary IS that member from the protocol's perspective.

Mitigations live at the **key management layer** (also app/user responsibility):

- Hardware-backed key storage (TEE, secure enclave, hardware wallet)
- Key rotation policies (out-of-scope for this RFC; orthogonal feature)
- Per-device sub-keys with separate revocation (also orthogonal; adds delegation surface)

If you suspect compromise, the in-protocol response is `MemberRemoved` (admin removes the compromised identity), which then triggers all the layered defenses above.

#### 8.4.3 Owner key compromise

**The trust assumption itself.** Calimero builds on Owner-as-trust-root (decision #13). If Owner's key is compromised, the whole namespace is compromised — including any key rotations, snapshots, transfers, or grants signed during the compromise window.

Decision #23 (TEE redundancy load-bearing) **raises the bar** but doesn't eliminate this:

- Multiple TEE-attested replicas can co-sign snapshots and serve as fallback sync sources
- Owner key compromise alone is not catastrophic if TEE attestations independently vouch for the same state
- Adversary now needs to compromise Owner *and* break the TEE attestation chain to mint canonical state

Practical hardening:

- **Hardware-backed Owner key** (HSM, TEE) is strongly recommended for production deployments
- **Multi-TEE replica deployments** for any namespace where Owner availability or compromise resistance is non-trivial
- **Owner key rotation** via `TransferOwnership` (existing, signed by current Owner) — limits damage if compromise is detected

This is the same trade-off as any PKI: ultimate trust roots are points of catastrophic failure if compromised; mitigations focus on making compromise hard and detectable, not impossible.

#### 8.4.4 Simultaneous compromise of all TEE attestations

**Extremely unlikely but possible.** If every TEE in the deployment is compromised AND Owner is compromised, decision #22's trust anchor falls. No protocol mechanism survives this.

Practical hardening:

- **Diverse TEE vendors** (Intel SGX + AMD SEV + Apple Secure Enclave) — single-vendor compromise doesn't break the attestation chain
- **Geographic / jurisdictional diversity** for TEE replicas
- **Periodic re-attestation** so a TEE compromise has a finite freshness window

These are deployment hardening recommendations, not protocol features.

#### 8.4.5 Application-side replays of valid old data via legitimate writes

**Out of scope for cross-DAG authz.** If a member legitimately writes "alice voted yes" twice (with two different valid HLCs), the protocol applies both — but the application has to decide whether that's idempotent or a bug.

Mitigations live at the **application data model** layer (same as §8.4.1):

- Use a `Set` instead of a counter for "has voted" semantics
- Use idempotent ops (`upsert`-style)
- Application-side dedup via a content-keyed log

#### 8.4.6 Out-of-band attacks (denial of service against the network, censorship at the libp2p layer, social engineering)

Not authorization concerns. libp2p / gossipsub provides standard mitigations (peer scoring, mesh diversity, IHAVE/IWANT message reconciliation). Social engineering and OOB attacks are user-education concerns.

### 8.5 Summary

| Threat class | Coverage |
|---|---|
| Malicious active member, protocol-level attacks | ✅ Fully covered (B3 + signature + role + forward-only + D4 + key rotation) |
| Malicious removed member, protocol-level attacks | ✅ Fully covered except the long-range attack residual, which is **bounded** by decisions #22 + #8 + #25 + D1 |
| Application-layer "valid-but-harmful" data | ⚠️ Out of scope; mitigated by data structure design (§8.4.1) |
| End-user key compromise | ⚠️ Out of scope; mitigated by key management hygiene (§8.4.2) |
| Owner key compromise | ⚠️ Trust assumption; **bar raised** by TEE redundancy (decision #23); mitigated by hardware-backed keys (§8.4.3) |
| Simultaneous TEE attestation compromise | ⚠️ Trust assumption; mitigated by diverse hardware + re-attestation (§8.4.4) |
| Application-side semantic replays | ⚠️ Out of scope; data structure design problem (§8.4.5) |
| Out-of-band attacks (DoS, social) | ⚠️ Not authorization concerns; libp2p + user education (§8.4.6) |

The honest framing: **all protocol-level attack surfaces are covered or bounded by design.** The remaining risks are application-layer concerns where Calimero deliberately delegates to the application's data model, or trust-assumption concerns where the design RAISES the bar (decision #23) but cannot eliminate the inherent risk of any cryptographic trust root.
