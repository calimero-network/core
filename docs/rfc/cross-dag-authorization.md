# RFC — Cross-DAG Authorization & Convergence

| | |
|---|---|
| **Status** | Draft / context-preservation |
| **Date** | 2026-05-06 |
| **Authors** | sandi@calimero.network |
| **Scope** | Governance DAG ↔ State DAG coordination, member removal semantics, convergence detection, DoS surface |
| **Related** | [#2233](https://github.com/calimero-network/core/issues/2233) (DAG-causal Shared verification), [#2237](https://github.com/calimero-network/core/issues/2237) (governance broadcast core), [#2280](https://github.com/calimero-network/core/pull/2280) (leave operations + Owner role), [#2284](https://github.com/calimero-network/core/pull/2284) (state hash + leave_group guard), [`docs/adr/0001-shared-storage-concurrent-rotation.md`](../adr/0001-shared-storage-concurrent-rotation.md) |

> **Status note.** This RFC captures the architectural context behind several open design questions surfaced while implementing leave_*, the e2e workflows, and the `wait_for_governance_sync` follow-up. Nothing here is implemented; the doc exists so future work has a single place to start from. The "categorized issues" section below is the candidate decomposition into actionable work — each could become its own GitHub issue, or together they could form one big tracking epic.

---

## 1. Summary

Calimero today runs two logically independent DAGs per group: a **governance DAG** (membership/role ops) and a **state DAG** (CRDT writes). The two are not causally cross-referenced. As a result:

- A removed member's writes can be applied on a peer that has not yet observed `MemberRemoved`, then propagate further as legitimate descendants build on them. Storage diverges between fast and slow peers.
- For `User`-storage actions there is **no membership check** at receive time — only signature + nonce. A removed member who keeps the old key can keep authoring valid-looking writes indefinitely.
- For `Shared`-storage actions there is a causally-aware writer-set check (rotation log + `writers_at(parents)`), but it is not automatically driven by `MemberRemoved` — only by explicit rotation actions inside `CausalDelta`s.
- The wire-format field `governance_epoch` exists on every state delta but is populated as `vec![]` by senders and ignored by receivers since #2237 Phase 11.2 — the cross-DAG coupling was removed for liveness reasons and not replaced.
- There is no API surface for a **governance state hash** comparable to `rootHash` — so e2e workflows fall back to fixed `wait, seconds: N` when they need to confirm governance has propagated.

This document lays out the problem precisely (with code references), proposes the design that would close it, and decomposes the work into four categories with size/dependency annotations.

---

## 2. Background — the two DAGs

### 2.1 The two hashes

| | State sync | Governance sync |
|---|---|---|
| **Covers** | WASM-level storage entries | group meta + members + roles + admin/owner identity + target app |
| **Storage column** | `Column::Identity` | `Column::Group*` |
| **Mutated via** | state DAG (`ContextDagDelta`) | governance DAG (`NamespaceOp` / `GroupOp`) |
| **Hash primitive** | `Snapshot::root_hash` (`crates/storage/src/snapshot.rs:35`) — Merkle root over storage entries | `compute_group_state_hash` (`crates/context/src/group_store/meta.rs:75`) — SHA-256 over members + roles + admin + owner + app |
| **API surface** | `rootHash` on `GET /admin-api/contexts/:ctx_id` | **none today** |

### 2.2 What `MemberRemoved` actually does in code

`crates/context/src/group_store/mod.rs:807-829` — the apply path does three things:

1. `cascade_remove_member_from_group_tree` — walks every context in the subtree and deletes the corresponding `ContextIdentity` row.
2. `remove_group_member` — deletes the `GroupMember` row.
3. Fires `OpEvent::MemberRemoved` → triggers the key rotation pipeline (new group key K1, wrapped only for current members).

The deleted `ContextIdentity` row is used for "do I have a private key for this context?" — it is **not** consulted on incoming state deltas. Its absence does not cause subsequent deltas from the removed member to be rejected.

### 2.3 What the state-delta receive path checks today

`crates/node/src/handlers/state_delta/mod.rs:67` (`handle_state_delta`) runs this membership check:

```rust
if calimero_context::group_store::is_read_only_for_context(
    node_clients.context.datastore(),
    &context_id,
    &author_id,
).unwrap_or(false) {
    // reject
}
```

`is_read_only_for_context` (`crates/context/src/group_store/namespace.rs:64`):

```rust
match get_group_member_role(store, &group_id, identity)? {
    Some(ReadOnly | ReadOnlyTee) => Ok(true),   // reject
    _ => Ok(false),                              // pass through
}
```

A removed member returns `None` from `get_group_member_role`, falls into the `_ => false` arm, **passes the check**. Only `ReadOnly` / `ReadOnlyTee` roles trigger rejection; "not a member" is treated identically to "Admin/Member".

### 2.4 Per-action verification (`crates/storage/src/interface.rs::apply_action`)

| Storage type | Check | Lines |
|---|---|---|
| `User` (single-owner) | Ed25519 verify against the action's claimed `owner` + nonce monotonicity. **No membership check.** | 268–330 |
| `Shared` (multi-writer CRDT) | Signature verified against `ctx.effective_writers`, resolved at receive via `writers_at(delta.parents)` per [ADR-0001](../adr/0001-shared-storage-concurrent-rotation.md) / #2266. Causally aware. | 340–410 |

The `Shared`-arm causal awareness comes from rotation log entries. Those are appended only from explicit rotation actions in `CausalDelta`s — there is **no automatic link from a governance `MemberRemoved` op to a rotation entry** on every Shared entity the removed member could write to. Coverage is limited to rotations the application explicitly performs.

### 2.5 The `governance_epoch` field — wire-format-only since #2237 Phase 11.2

`crates/node/src/handlers/state_delta/mod.rs:33` deserializes `governance_epoch: Vec<[u8; 32]>` from incoming state deltas. This was originally the cross-DAG reference. But:

- `crates/context/src/handlers/execute/mod.rs:731` populates it as `vec![]` (empty) on the sender side.
- `state_delta/mod.rs:122-131` says: *"Governance catch-up is now handled by the namespace heartbeat (`NamespaceStateHeartbeat`). The `governance_epoch` field on state deltas is retained for informational logging only."*
- `NamespaceStateHeartbeat` itself (`crates/node/src/handlers/network_event/namespace.rs:192`) is **liveness-only** as of #2237 Phase 11.2 — it logs divergence at debug level and does not take action.

So the field exists in the wire format but is functionally dead.

---

## 3. Concrete divergence scenarios

### 3.1 Apply-lag race

Group {A, B, X}. A publishes `MemberRemoved {X}`.

| | Node A | Node B |
|---|---|---|
| T0 | publishes `MemberRemoved {X}`; state op S' from X is in flight | — |
| T1 | applies `MemberRemoved`, cascade deletes `ContextIdentity`, rotates K0 → K1 | receives S' (still has K0), checks signer X, `get_group_member_role` returns `Member`, **applies S'** |
| T2 | receives S' encrypted with K0 (gossip layer doesn't drop) — but A no longer has X's `ContextIdentity` — actually A still applies because the receive-path doesn't check membership | receives `MemberRemoved`, applies, rotates key |

End state: A's storage and B's storage may differ depending on the precise message ordering and decryption availability. `root_hash(A) ≠ root_hash(B)`. Permanent divergence — there's no rollback mechanism.

### 3.2 Cross-partition state propagation

Group {A, B, C}. B has shared state deltas with C that A hasn't yet observed. A proposes `MemberRemoved {B}`:

1. A's local view of B's state-DAG heads is incomplete.
2. If `MemberRemoved` declares an enumerated cut `cut_dag_heads = [A's view of B's heads]`, B's writes that propagated to C are not in the cut's ancestry.
3. Once partition heals, A receives those deltas via C. Each is "after the cut" by enumeration → A rejects them. C accepts them. Permanent divergence, this time *caused by* the cut declaration itself.

This is why an enumerated-heads cut is the wrong primitive. The cut must be a **governance-DAG position**, not a state-DAG-heads enumeration.

### 3.3 Taint cascade

B applies illegal S from removed X before `MemberRemoved` arrives. B then publishes its own delta T with `parents=[S]`. T is signed by B (legitimate). Other peers receive T → fetch S → S also propagates. Once `MemberRemoved` reaches those peers, they would want to reject S, but T (signed by B) structurally depends on S. Rejecting S means rejecting T. CRDT commutativity does not help — the dependency graph entangles authorization and data.

---

## 4. Proposed design

### 4.1 `governance_position` on every state delta

Extend `ContextDagDelta` / `CausalDelta`:

```rust
pub struct GovernancePosition {
    pub group_id: ContextGroupId,
    pub group_state_hash: [u8; 32],     // from compute_group_state_hash
    pub governance_dag_heads: Vec<[u8; 32]>,
}
```

This is essentially restoring the original semantic of the existing `governance_epoch` field, with stronger content (state hash, not just heads). The sender computes `compute_group_state_hash` + governance DAG heads at sign time and embeds them in the delta.

### 4.2 Receivers buffer state deltas ahead of governance

On receive of a state delta whose `governance_position.dag_heads` references heads not yet observed locally:

- Buffer the delta. Do not apply.
- When local governance DAG reaches those heads (via normal sync / ack flows), check `compute_group_state_hash` matches the delta's claimed `group_state_hash`.
- If matches → safe to apply; the writer was authorized at a governance state we now share.
- If doesn't match → reject (Byzantine sender or stale claim).

### 4.3 Apply-time membership check uses governance_position, not local state

Replace today's *"is the signer currently a member?"* with *"was the signer a member at the governance state this delta references?"* The lookup uses governance state history (small ring buffer of recent group state hashes → membership snapshots; older ones can be derived by replaying ops to a checkpoint).

### 4.4 The cut for `MemberRemoved` is its position in the governance DAG

No `cut_dag_heads` field. The cut is implicit: it's the position of `MemberRemoved` in the governance DAG. The rule is:

> Delta `D` from removed member `X` is valid iff `D.governance_position` does not descend from `MemberRemoved` in the governance DAG.

This handles §3.2 cross-partition propagation correctly: B's writes from before the remove (any partition) carry `governance_position` that predates the cut → valid → merge.

### 4.5 Quorum-acked `MemberRemoved`

Single-admin async `MemberRemoved` applies at different times on different peers. Bounding the apply-lag window requires **quorum acknowledgment**:

- Admin proposes `MemberRemoved {X}`.
- Op carries no effect until M-of-N members ack it via the existing `publish_and_await_ack` infrastructure (`crates/context/src/governance_broadcast.rs`).
- Quorum proof (M signatures) is attached to the op. Receivers verify proof before considering the op applied.
- The cut becomes well-defined: governance DAG position of the quorum-acked op.

### 4.6 Snapshots / checkpoints

Periodic quorum-signed snapshots of group state cap rebuild scope. A peer that wrongly applied a delta during a race rebuilds from the most recent snapshot + valid deltas only, instead of from genesis. Required for bounded recovery. Also used as long-range-attack sealing (see §5.1.3).

---

## 5. Edge cases & attack surface

### 5.1 Long-range attack — removed member spam

After removal, a Byzantine ex-member can author new deltas indefinitely, all claiming `governance_position = H_old` (some pre-removal state). The signature is valid; the membership at H_old said they were a member. `governance_position` alone does not bound this. **Layered defenses:**

#### 5.1.1 Network-layer deny-list

On `MemberRemoved` apply, add the removed peer's libp2p ID to a blacklist for governance + state topics. Drops messages before they reach apply. Highest-leverage defense; stops the bulk of spam without any protocol change.

#### 5.1.2 Time-window validity

Delta is valid only if `delta.hlc < H_old.applied_at + W`, where W is a propagation budget (e.g. 24h). After W elapses, no new deltas claiming H_old can be authored. Bounds the long-tail spam window. Tradeoff: W must be wide enough for honest async propagation but narrow enough to bound spam.

#### 5.1.3 Snapshot sealing

Deltas claiming `governance_position` older than the most recent sealed snapshot are rejected outright. This is the long-range-attack defense from PoS chains (weak subjectivity checkpoints), applied to governance.

#### 5.1.4 Old-key (K0) deprecation

After `MemberRemoved` + grace period, drop K0-encrypted gossip at receive. Closes the "removed member writes with the old key" channel at the gossip layer.

#### 5.1.5 Per-peer rate limits

Bandwidth/msg-rate caps per signer at the gossip apply layer. Catches anything that slipped through earlier layers and bounds slow-leak attacks.

### 5.2 Concurrent governance ops across partitions

Two admins on disjoint partitions concurrently propose conflicting ops (e.g. one removes X, another promotes X to admin). Each can collect quorum from disjoint sets. After heal, the governance DAG has a fork. Resolution requires either:

- **HLC tiebreak on governance ops** — last-writer-wins on membership state, with the loser becoming a no-op.
- **Quorum-overlap requirement** — quorums must overlap by ≥1 member to prevent split-brain (Raft-style strict majority).

Calimero's existing `SignedGroupOp` divergence check (`compute_group_state_hash` embedded in the op for #2284) already gives fork *detection*. The resolution policy on detected concurrency is a deferred design call.

### 5.3 Honest writer with malicious-claimed `governance_position`

A Byzantine actor signs a delta claiming `governance_position = H_old` when in fact they observed the newer `H_remove`. There is no way to disprove the negative observation in async consensus. The defense is **detection, not prevention**: if the actor's other behavior contradicts the claim (acks of newer ops, beacons, etc.), forensic tooling can flag the inconsistency. Practically bounded by §5.1's layered defenses.

### 5.4 Authored-but-unobserved-cut (the C scenario)

A B C; A removes B; B has shared state with C that A doesn't have; A proposes the cut.

Under the proposed design, this resolves cleanly:

- B's writes that went to C carry `governance_position = H_old` (some pre-removal state).
- C eventually propagates them to A.
- A validates: `H_old` predates `MemberRemoved` in the governance DAG → valid → applies.
- Final state on A and C: identical.

The reason there's no divergence is that the validity criterion is stable across peers (a function of `governance_position` and the governance DAG, both of which all peers eventually agree on), rather than a function of "what the proposer happened to have observed at proposal time."

---

## 6. Categorized issues

Four categories, ten concrete issues. Sizes: **S** = ~1 week, **M** = 2–4 weeks, **L** = multi-month.

### A. Observability & convergence detection

Quick wins that unblock testing without changing wire format.

| # | Issue | Size | Depends on |
|---|---|---|---|
| **A1** | Expose `state_hash` on `GET /admin-api/groups/:group_id` by wiring `compute_group_state_hash` through `GroupInfoApiResponseData` (`crates/server/primitives/src/admin/mod.rs:84`) | S | — |
| **A2** | Add `wait_for_governance_sync` workflow step in merobox, polls `state_hash` across nodes (mirrors `WaitForSyncStep`) | M | A1 + release cascade |
| **A3** | Hierarchical Merkle: `NamespaceMerkle { meta, members, governance_dag_root, subgroups, contexts }` exposed via API for whole-subtree convergence checks | L | A1, B1, C2 |

### B. Cross-DAG causal authorization

The foundational change: state deltas reference governance state, receivers enforce it.

| # | Issue | Size | Depends on |
|---|---|---|---|
| **B1** | Add `governance_position` field to `ContextDagDelta` / `CausalDelta`. Wire-format breaking change. Replace today's empty-vec `governance_epoch`. | M | — |
| **B2** | Receiver-side: buffer state deltas whose `governance_position` references governance state not yet observed locally; release once governance catches up | M | B1 |
| **B3** | Apply-time check: `delta.signer` must have been a member in the governance state `delta.governance_position` claims. Replaces the partial `is_read_only_for_context` check. | M | B1, B2 |

### C. Removal semantics & recovery

Quorum + bounded recovery. Reuses existing `publish_and_await_ack` infrastructure.

| # | Issue | Size | Depends on |
|---|---|---|---|
| **C1** | Quorum-acked `MemberRemoved` / `MemberLeft` (M-of-N member signatures attached; receivers verify before considering applied) | M | — |
| **C2** | Snapshot/checkpoint mechanism at quorum boundaries — bounds rebuild scope when a peer needs to recover from wrongly-applied deltas | L | C1 |
| **C3** | Local rebuild tool: detect tainted state (deltas with rejected `governance_position`), re-derive from most recent valid snapshot + replay only valid deltas | M | C2, B3 |
| **C4** | Documented + enforced **forward-only** authorization semantic: writes authored from a pre-cut `governance_position` are honored regardless of arrival order, even on partitions the proposer didn't see | S | B3 |

### D. DoS & Byzantine defenses

Layered defenses against the long-range-attack surface.

| # | Issue | Size | Depends on |
|---|---|---|---|
| **D1** | Network-layer deny-list on `MemberRemoved` — drops gossipsub messages from removed peer at the libp2p layer before reaching apply. **Highest leverage.** | S | — |
| **D2** | Time-window validity on `governance_position`: `delta.hlc < governance_position.applied_at + W` | S | B1 |
| **D3** | Old-key (K0) deprecation policy after `MemberRemoved` + grace period; drop K0-encrypted gossip at receive after deprecation | S | C1 |
| **D4** | Per-peer rate limits at gossip apply layer — bandwidth / message-rate caps per signer | M | — |

---

## 7. Recommended landing order

### Phase 1 — Independent quick wins (parallel, ~1–2 weeks each)

1. **A1** — exposes governance state hash, immediately useful for testing
2. **A2** — workflow step type, follows A1 release cascade
3. **D1** — network deny-list on removal, immediate DoS surface reduction

### Phase 2 — Foundational lift (sequential, ~1–2 months total)

4. **B1** — `governance_position` wire format change; breaking, coordinate with release
5. **B2 + B3** — buffering + apply-time validation
6. **C4** — declare forward-only semantic now that the primitives exist

### Phase 3 — Quorum + recovery (sequential, ~1–2 months)

7. **C1** — quorum acks on `MemberRemoved` / `MemberLeft` (reuses existing ack-router infra)
8. **D2 + D3** — time-window + key deprecation, layered defense
9. **C2 + C3** — snapshots + rebuild tool

### Phase 4 — Long-tail / vision

10. **A3** — hierarchical Merkle once everything else lands; gives whole-subtree convergence detection in one hash
11. **D4** — rate limits, only if there's empirical evidence of a slow-leak attack surface

---

## 8. Summary — what Calimero today does NOT do

1. **Governance state hash is not exposed on the admin API.** Tests use `type: wait, seconds: N` for governance because there is no equivalent of `rootHash` for `get_group_info`.
2. **State deltas do not reference the governance state they were authorized against.** The `governance_epoch` field exists in the wire format but is populated as `vec![]` by senders and ignored by receivers since #2237 Phase 11.2.
3. **Receiver-side membership check on state deltas is partial.** Only `ReadOnly` / `ReadOnlyTee` roles are rejected; outright removal results in `None` from `get_group_member_role`, which the apply path treats as "pass through."
4. **For `User`-storage actions there is no membership check at all** — only Ed25519 signature + nonce replay protection.
5. **The `Shared`-storage causal writer-set check is not automatically connected to `MemberRemoved`** — rotation log entries come from explicit rotation actions in `CausalDelta`s, not from governance ops.
6. **No quorum on `MemberRemoved`.** Single-admin async apply with unbounded propagation lag across peers.
7. **No deterministic cut declaration.** The cut, where it exists conceptually, is each peer's local apply-time of the op — different on every peer.
8. **No network-layer deny-list on removal.** A removed member's gossipsub messages continue to be processed.
9. **No time-window validity** on signatures from removed members — old governance positions remain valid claims indefinitely.
10. **No snapshot mechanism** to bound rebuild scope or seal the governance history against long-range claims.

The mechanisms that *do* bound divergence today:

- **Key rotation on `MemberRemoved`** — closes interactive protocols (the removed member can't read responses encrypted with K1). Does not stop one-way writes encrypted with K0 during the grace window.
- **Eventually-detected `root_hash` divergence** — operators can grep `NamespaceStateHeartbeat` debug logs for mismatch. No automatic remediation.
- **`SignedGroupOp.current_state_hash` divergence check** — rejects governance ops signed against a stale group state. This is the governance DAG's *own* internal divergence prevention; it does not touch the state DAG.

---

## 9. Related work in the codebase

- **#2237 governance broadcast core** — `publish_and_await_ack` + `AckRouter` + readiness FSM. Infrastructure that quorum-acked `MemberRemoved` (issue C1) would build on.
- **#2266 DAG-causal Shared verifier** — `writers_at(parents)` + rotation log. The receiver-side causal authorization model for Shared CRDTs. Issue B3 is essentially extending this to `User`-storage actions and to all writers (not just rotation actions). [ADR-0001](../adr/0001-shared-storage-concurrent-rotation.md) is the formal spec.
- **#2284 state hash includes owner_identity** — `compute_group_state_hash` covers members + roles + admin + owner + app. The hash issue A1 would expose is the same primitive.
- **[Membership & Leave](../../architecture/membership-and-leave.html)** — formalizes the Owner role and self-leave operations. `MemberLeft` shares the same authorization gap as `MemberRemoved`; the cross-DAG fixes here apply to both.

---

## 10. Open questions for the team

1. **Wire-format breaking change strategy.** B1 changes the on-wire shape of `ContextDagDelta`. How should this be sequenced with releases? Versioned protocol message? Hard cut at a release boundary?
2. **Quorum size and member set semantics.** For C1, is M-of-N over the *group's member set* at the proposal's `governance_position`? What happens when the group only has 1–2 members (M=1 trivially, no quorum value)?
3. **Snapshot frequency and trigger.** For C2, are snapshots periodic (every N ops, every T seconds) or admin-triggered? Who signs them?
4. **Forward-only vs retroactive default.** C4 declares forward-only as the default. Should retroactive invalidation be an opt-in mode for high-stakes deployments, or out of scope entirely?
5. **Time-window W.** D2 needs a concrete value. 1h is too tight for hostile networks; 7 days is too lax for hostile actors. Probably 24h is the working default, but should it be per-namespace configurable?
6. **Network deny-list scope.** D1 — is the deny-list per-group, per-namespace, or global per-peer-id? What if the same libp2p peer is a member of group X (where they were removed) and group Y (where they're still active)?
7. **Epic vs separate issues.** The 10 issues here decompose roughly into Phase 1 (3 issues) → Phase 2 (3 issues) → Phase 3 (3 issues) → Phase 4 (1+ issues). Should they be tracked as one umbrella epic with sub-issues, or four separate epics by category, or ten standalone issues?

---

## 11. Out of scope (intentionally not addressed here)

- Token-economic incentives for voting / quorum participation — Calimero is not a public chain.
- BFT-style consensus on every governance op — would defeat the point of CRDT-style permissionless writes within the member set.
- Full retroactive invalidation of removed-member contributions to CRDT state — fundamentally hard in async CRDT-with-access-control settings; the "forward-only" semantic in C4 is the realistic stance.
- Hardware-backed identity (TEE-anchored signatures beyond what `ReadOnlyTee` already provides).
