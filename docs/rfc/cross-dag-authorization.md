# RFC — Cross-DAG Authorization & Convergence

| | |
|---|---|
| **Status** | Draft |
| **Date** | 2026-05-07 |
| **Authors** | sandi@calimero.network |
| **Scope** | Governance DAG ↔ State DAG coordination, member removal & leave semantics, convergence detection, eclipse / DoS surface |
| **Related** | [#2233](https://github.com/calimero-network/core/issues/2233) (DAG-causal Shared verification), [#2237](https://github.com/calimero-network/core/issues/2237) (governance broadcast core), [#2280](https://github.com/calimero-network/core/pull/2280) (leave operations + Owner role), [#2284](https://github.com/calimero-network/core/pull/2284) (state hash + leave_group guard), [`docs/adr/0001-shared-storage-concurrent-rotation.md`](../adr/0001-shared-storage-concurrent-rotation.md) |

> **Status note.** This RFC captures the architectural context behind several open design questions surfaced while implementing leave_*, the e2e workflows, and the `wait_for_governance_sync` follow-up. Nothing here is implemented; the doc exists so future work has a single place to start from. The "categorized issues" section below is the candidate decomposition into actionable work — each could become its own GitHub issue, or together they could form one tracking epic.

---

## 1. Summary

Calimero today runs two logically independent DAGs per group: a **governance DAG** (membership/role ops) and a **state DAG** (CRDT writes). They are not causally cross-referenced. As a result:

- A removed member's writes can be applied on a peer that has not yet observed `MemberRemoved`, then propagate further as legitimate descendants build on them. Storage diverges.
- For `User`-storage actions there is **no membership check** at receive time — only signature + nonce.
- For `Shared`-storage actions there is a causally-aware writer-set check, but it is not automatically driven by `MemberRemoved` — only by explicit rotation actions.
- The wire-format field `governance_epoch` exists on every state delta but is sent as `vec![]` and ignored on receive since #2237 Phase 11.2.
- There is no API surface for a **governance state hash** comparable to `rootHash` — so e2e workflows fall back to `wait, seconds: N` to confirm governance has propagated.

The proposed design closes this gap with a single primitive — `governance_position` on every state delta — combined with a deterministic governance-DAG cut for removal, network-layer isolation of removed peers, and an Owner/TEE-anchored bootstrap to prevent eclipse on join. **Quorum/voting schemes are explicitly out of scope** as a design choice — they don't fit Calimero's CRDT-style permissionless-within-the-set model and create a liveness regression for small groups and partitions.

---

## 2. Goals & Non-goals

### Goals

1. **Convergence between honest peers.** Apply-lag, partition heal, and arrival-order variations must never produce a permanent fork between honest peers running the same protocol.
2. **Bounded effect of a removed member.** After `MemberRemoved`, the removed peer's ability to inject content into the group must be cut off within gossip-propagation time, not "indefinitely."
3. **Causally-stable validity rule.** Whether a state delta is valid must be a pure function of `(delta.governance_position, canonical governance DAG)` — same answer on every peer, no dependence on local clock or apply-time.
4. **Forward-only semantic.** Pre-cut writes from a removed member are valid regardless of arrival order. Post-cut writes are rejected. No retroactive invalidation of legitimately-authored history.
5. **Eclipse-resistant bootstrap.** A late-joining peer cannot be silently fed a stale or doctored governance view by a malicious peer.
6. **Convergence detection in tests.** e2e workflows must be able to wait deterministically on governance state, not by sleep.

### Non-goals

1. **Closing the long-range surface entirely.** A Byzantine ex-member who keeps their key can sign valid-looking deltas claiming a pre-removal `governance_position`. This is fundamentally undecidable in async consensus (no global clock, no way to disprove a negative observation). The design **bounds** this — to gossip-propagation time of `MemberRemoved` plus partition duration — rather than preventing it.
2. **Quorum / M-of-N voting / multisig-style approval.** Off the table. See §1 and §5.1 for rationale. Resolution mechanisms must be deterministic-causal (governance-DAG positions) or role-hierarchical (Owner override), not vote-based.
3. **Token-economic incentives or BFT consensus on every governance op.** Calimero is not a public chain.
4. **Hardware-backed identity** beyond what `ReadOnlyTee` already provides.

---

## 3. Background — what Calimero does today

### 3.1 The two hashes

| | State sync | Governance sync |
|---|---|---|
| **Covers** | WASM-level storage entries | group meta + members + roles + admin/owner identity + target app |
| **Storage column** | `Column::Identity` | `Column::Group*` |
| **Mutated via** | state DAG (`ContextDagDelta`) | governance DAG (`NamespaceOp` / `GroupOp`) |
| **Hash primitive** | `Snapshot::root_hash` (`crates/storage/src/snapshot.rs:35`) — Merkle root over storage entries | `compute_group_state_hash` (`crates/context/src/group_store/meta.rs:75`) — SHA-256 over members + roles + admin + owner + app |
| **API surface** | `rootHash` on `GET /admin-api/contexts/:ctx_id` | **none today** |

### 3.2 What `MemberRemoved` actually does in code

`crates/context/src/group_store/mod.rs:823-825` — the apply path does three things:

1. `cascade_remove_member_from_group_tree` — walks every context in the subtree and deletes the corresponding `ContextIdentity` row.
2. `remove_group_member` — deletes the `GroupMember` row.
3. Fires `OpEvent::MemberRemoved` → triggers the key rotation pipeline (new group key K1, wrapped only for current members).

The deleted `ContextIdentity` row is used for "do I have a private key for this context?" — it is **not** consulted on incoming state deltas. Its absence does not cause subsequent deltas from the removed member to be rejected.

### 3.3 What the state-delta receive path checks today

`crates/node/src/handlers/state_delta/mod.rs:96` (`handle_state_delta`) runs this membership check:

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

A removed member returns `None` from `get_group_member_role`, falls into the `_ => false` arm, **passes the check**. Only `ReadOnly` / `ReadOnlyTee` roles trigger rejection; "not a member" is treated identically to "Admin/Member."

### 3.4 Per-action verification (`crates/storage/src/interface.rs::apply_action`)

| Storage type | Check | Lines |
|---|---|---|
| `User` (single-owner) | Ed25519 verify against the action's claimed `owner` + nonce monotonicity. **No membership check.** | 277–330 |
| `Shared` (multi-writer CRDT) | Signature verified against `ctx.effective_writers`, resolved at receive via `writers_at(delta.parents)` per [ADR-0001](../adr/0001-shared-storage-concurrent-rotation.md) / #2266. Causally aware. | 339–410 |

The `Shared`-arm causal awareness comes from rotation log entries. Those are appended only from explicit rotation actions in `CausalDelta`s — there is **no automatic link from a governance `MemberRemoved` op to a rotation entry** on every Shared entity the removed member could write to. Coverage is limited to rotations the application explicitly performs.

### 3.5 The `governance_epoch` field — wire-format-only since #2237 Phase 11.2

`crates/node/src/handlers/state_delta/mod.rs:33` deserializes `governance_epoch: Vec<[u8; 32]>` from incoming state deltas. This was originally the cross-DAG reference. But:

- `crates/context/src/handlers/execute/mod.rs:731` populates it as `vec![]` on the sender side.
- `state_delta/mod.rs:122-131` says: *"Governance catch-up is now handled by the namespace heartbeat (`NamespaceStateHeartbeat`). The `governance_epoch` field on state deltas is retained for informational logging only."*
- `NamespaceStateHeartbeat` itself (`crates/node/src/handlers/network_event.rs:189`) is **liveness-only** as of #2237 Phase 11.2 — it logs divergence at debug level and does not take action.

So the field exists in the wire format but is functionally dead.

---

## 4. Divergence scenarios

### 4.1 Apply-lag race

Group {A, B, X}. A publishes `MemberRemoved {X}`.

| | Node A | Node B |
|---|---|---|
| T0 | publishes `MemberRemoved {X}`; state op S' from X is in flight | — |
| T1 | applies `MemberRemoved`, cascade deletes `ContextIdentity`, rotates K0 → K1 | receives S' (still has K0), checks signer X, `get_group_member_role` returns `Member`, **applies S'** |
| T2 | receives S' encrypted with K0 — A still applies because the receive path doesn't check membership | receives `MemberRemoved`, applies, rotates key |

End state: A's storage and B's storage may differ depending on the precise message ordering and decryption availability. `root_hash(A) ≠ root_hash(B)`. Permanent divergence — there's no rollback mechanism today.

#### 4.1.1 Why isn't K0 → K1 rotation alone sufficient?

A natural objection: *the key is rotated when `MemberRemoved` applies — the removed member can't encrypt to the new key, so the explicit membership check is redundant.* It isn't. The encryption-layer defense and the membership-layer defense fail at different times:

1. **Rotation itself is async.** At T1 above, A is on K1 but B is still on K0. X's delta S' encrypted with K0 decrypts cleanly on B; the receive path runs `is_read_only_for_context` against a still-present `Member` row, hits the `_ => false` arm, and **applies S'**. The window during which at least one peer is post-rotation while at least one is pre-rotation is exactly the apply-lag.

2. **K1 rotation is an interactive-channel defense.** It closes "X reads responses encrypted under the new key" — i.e. it stops X from participating in two-way protocols. It does not stop one-way K0-encrypted writes during the grace window.

3. **Causal entanglement makes the window permanent.** Once B has applied S', B publishes its own legitimate delta T with `parents=[S']`. T is signed by B with K1; T propagates to A via the normal gossip path and to other peers post-rotation. Rejecting S' after the fact means rejecting T (and everything causally downstream of T) — see §4.3. So a single delta admitted during the lag window can taint an unbounded subgraph of post-rotation, legitimately-signed history.

4. **Partition-heal paths bring K0-era messages through non-rotated peers.** §4.2 spells this out: if C is on a partition that hasn't yet seen `MemberRemoved`, C accepts and re-broadcasts X's K0 messages. Once A↔C heals, the messages arrive at A through C's still-K0 view. Encryption-layer rotation on A doesn't help when the message reached A via a peer that legitimately still had K0.

### 4.2 Cross-partition state propagation

Group {A, B, C}. B has shared state deltas with C that A hasn't yet observed. A proposes `MemberRemoved {B}`:

1. A's local view of B's state-DAG heads is incomplete.
2. If `MemberRemoved` declared an enumerated cut `cut_dag_heads = [A's view of B's heads]`, B's writes that propagated to C are not in the cut's ancestry.
3. Once partition heals, A receives those deltas via C. Each is "after the cut" by enumeration → A rejects them. C accepts them. Permanent divergence, this time *caused by* the cut declaration itself.

This is why an enumerated-heads cut is the wrong primitive. The cut must be a **governance-DAG position**, not a state-DAG-heads enumeration.

### 4.3 Taint cascade

B applies illegal S from removed X before `MemberRemoved` arrives. B then publishes its own delta T with `parents=[S]`. T is signed by B (legitimate). Other peers receive T → fetch S → S also propagates. Once `MemberRemoved` reaches those peers, they would want to reject S, but T (signed by B) structurally depends on S. Rejecting S means rejecting T. CRDT commutativity does not help — the dependency graph entangles authorization and data.

---

## 5. Proposed design

The design is built on a single primitive (`governance_position`) plus a deterministic cut declaration. There is no synchronous gate, no quorum, no global clock dependency.

### 5.1 Why no quorum

Quorum-acked `MemberRemoved` was an obvious-looking solution to "bound the apply-lag window," and is explicitly rejected here:

- **Liveness regression.** Quorum requires M responsive members. Offline / partitioned members stall removal. This breaks the CRDT-style permissionless-within-the-set model the rest of Calimero relies on.
- **Doesn't fit small groups.** N=1 and N=2 groups have no meaningful M-of-N value.
- **Doesn't actually solve the underlying problem.** §5.4's apply-time check using `governance_position` already gives correctness regardless of apply-lag. Quorum was solving a *latency-bounding* problem, and §6 layered defenses bound that latency without a synchronous gate.
- **K-of-K admin signatures (the "lighter" multi-admin variant)** has the same shape and is also out of scope.

The right primitives are **deterministic causal** (governance-DAG position), **per-actor signed** (admin signs the cut, leaver signs their own departure), and **role-hierarchical** (Owner can reverse Admin actions). All three are detailed below.

### 5.2 `governance_position` on every state delta

Extend `ContextDagDelta` / `CausalDelta`:

```rust
pub struct GovernancePosition {
    pub group_id: ContextGroupId,
    pub group_state_hash: [u8; 32],        // from compute_group_state_hash
    pub governance_dag_heads: Vec<[u8; 32]>,
}
```

This restores the original semantic of the dead `governance_epoch` field with stronger content (state hash, not just heads). Senders compute `compute_group_state_hash` + governance DAG heads at sign time and embed them in the delta. Wire-format breaking change — replaces the empty-vec `governance_epoch`.

### 5.3 Buffer on unknown governance state

On receive of a state delta whose `governance_position.governance_dag_heads` references heads not yet observed locally:

- **Buffer the delta. Do not apply.**
- When the local governance DAG reaches those heads (via normal sync), check `compute_group_state_hash` matches the delta's claimed `group_state_hash`.
- If matches → safe to apply. The writer was authorized at a governance state we now share.
- If doesn't match → reject. Byzantine sender or stale claim.

This is what makes the validity rule a pure function of `(delta.governance_position, governance DAG)`: receivers never apply a delta whose governance reference is undecidable. They wait until it is.

### 5.4 Apply-time membership check uses `governance_position`, not local state

Replace today's *"is the signer currently a member?"* with *"was the signer a member at the governance state this delta references?"* The lookup uses governance state history (a small ring buffer of recent group state hashes → membership snapshots; older states can be derived by replaying ops to a checkpoint).

This subsumes `is_read_only_for_context` and makes the membership check work for `User`-storage actions too (which today have no membership check at all).

### 5.5 Cut declarations: admin-signed (forced) and self-signed (voluntary)

Two distinct flows, both producing a deterministic cut:

**Forced removal (admin-signed deterministic cut).** The admin includes the cut position explicitly inside the signed `MemberRemoved` op:

```rust
pub struct MemberRemovedOp {
    pub group_id: ContextGroupId,
    pub removed_member: PublicKey,
    pub cut: GovernancePosition,           // embedded — signed by admin
    pub admin_signature: Signature,
    // ... existing fields
}
```

The validity rule for state deltas from `removed_member`:

> Delta `D` from `X` is valid iff `D.governance_position` does not descend from `MemberRemovedOp.cut` in the governance DAG.

Because the cut is embedded (not implicit at `position_of(MemberRemoved)` in each peer's DAG view), every peer evaluates the same rule against the same cut value, even if their governance DAG has not fully converged. No quorum needed.

**Voluntary leave (self-signed).** `MemberLeft` is signed by the leaver themselves and carries their own `governance_position` at sign time. There is no admin gating; the leaver attests "from this position onward, I am no longer participating." Every peer applies the same forward-only rule with the leaver-signed cut. This is the easy case — the leaver is honest by definition (a Byzantine leaver who continues authoring is the same surface as a Byzantine ex-member, handled by §6).

### 5.6 Forward-only semantic

For both forced removal and self-leave: **deltas authored from a pre-cut `governance_position` are valid forever, regardless of arrival order or partition path.** They are never retroactively invalidated. Reasons:

- Retroactive invalidation entangles authorization with CRDT data (§4.3 taint cascade) and has no convergent answer in async settings.
- Forward-only is the realistic stance — also taken by every async access-control-CRDT system in the literature.
- The bound on long-range "valid pre-cut" spam is provided by §6 layered defenses, not by the validity rule itself.

### 5.7 Owner override as recovery

Single-admin async `MemberRemoved` can be wrong (a Byzantine admin removes a legitimate member). The recovery path is **role-hierarchical**: Owner can sign a `MemberRestored` op that supersedes the admin's `MemberRemoved`. The restored member's pre-`MemberRemoved` history is unaffected by the spurious removal (forward-only; their writes were valid at their `governance_position`). Their post-removal writes — buffered or rejected during the window — get re-evaluated once `MemberRestored` propagates.

This replaces what a quorum design would have offered ("multiple admins must agree before removal takes effect") with a hierarchical alternative ("Owner can reverse Admin"). It does not require multiple admins to be online; it only requires Owner to be reachable when reversal is needed.

---

## 6. Long-range attack surface & layered defenses

### 6.1 The fundamental impossibility

A Byzantine ex-member X who keeps their old signing key can author state deltas claiming `governance_position = H_old` (any pre-removal state). The signature is valid; X was a real member at H_old. There is no externally-verifiable distinction between:

- **Honest:** X authored at wall-clock T, before observing `MemberRemoved`.
- **Byzantine:** X authored at T'>T, after observing `MemberRemoved`, lying about timing.

Async consensus has no global clock, and X's claim is cryptographically self-consistent. This is the standard **long-range attack** in PoS / asynchronous systems and is not Calimero-specific. The §5 design **cannot prevent** this; it can only **bound** it.

### 6.2 Load-bearing defense — D1 network-layer deny-list

On `MemberRemoved` apply, every peer adds the removed member's identity to a deny-list and **drops gossip messages whose inner Calimero signer matches** at the libp2p apply boundary (after decryption, before further processing). Critical implementation detail: the deny-list is keyed on **signer identity** (the Calimero-layer key inside the message), not on libp2p peer-id — otherwise X can rotate their libp2p peer-id and bypass.

With universal D1, the long-range attack surface collapses to gossip-propagation time of `MemberRemoved`:

| Window | Bound |
|---|---|
| Healthy mesh | Gossip propagation of `MemberRemoved` (seconds) |
| Network partition | Partition heal time + propagation time |
| Long-offline rejoining peer | Reconnects to live peers → receives `MemberRemoved` first → enables D1 before processing X-authored content |

Anything X did slip through during these windows carries a pre-cut `governance_position` (X authored before peers had `MemberRemoved` to deny-list against), so by §5.6 forward-only, the resulting deltas converge identically on every honest peer. **No fork.**

Promote D1 from "highest leverage" to "the actual answer." It is not optional.

### 6.3 Defense-in-depth — D2, D3, D4

**D2 — time-window validity.** `delta.hlc < governance_position.applied_at + W`. After window W elapses, no new deltas claiming that position are accepted. Largely redundant with D1 if D1 is universal and correctly keyed on signer identity, but worth keeping as belt-and-suspenders against D1 implementation bugs. Reasonable W: 24h, per-namespace configurable.

**D3 — old-key (K0) deprecation.** After `MemberRemoved` + grace period, drop K0-encrypted gossip at receive. Defends against malicious *active* members who haven't been removed but are using K0 maliciously after a rotation triggered by some other event — distinct surface from "removed member," still earns its keep.

**D4 — per-peer rate limits.** Bandwidth / msg-rate caps per signer at the gossip apply layer. Generic gossipsub hygiene; only worth implementing if empirical evidence of slow-leak attacks emerges.

### 6.4 Bootstrap & eclipse resistance

A late-joining peer P that knows only one peer faces an **eclipse attack**: that peer can serve P a stale or doctored governance view (omit `MemberRemoved {X}`, claim X is still a member). P's view ends up stale, not divergent — anything P applies under the stale view has pre-cut `governance_position` and is forward-only-valid → converges with the rest of the network once P escapes the eclipse. But until escape, P is vulnerable to applying X's content.

**X cannot forge** governance ops (every op is signed by its actual author, chain back to genesis verifiable locally). The only attack X can mount as a bootstrap source is **censorship** — a strict prefix of the honest DAG.

The bootstrap design pins to the **role hierarchy**, leveraging libp2p's built-in peer-id authentication:

**Primary trust anchor — Owner / TEE peer.** P knows the Owner's peer-id from group genesis (it's part of `compute_group_state_hash`, fixed at creation, verifiable on every governance op). P dials the Owner by peer-id; libp2p's handshake authenticates that the responder holds the corresponding private key. X cannot impersonate Owner. The Owner returns the current member set, current state hash, and current governance DAG heads. P now has an authoritative anchor; from there P fetches full state from any peer in the returned member set and verifies against the anchor's state hash.

TEE peers are a natural redundancy layer (`ReadOnlyTee` already exists in the role model). Multiple TEEs can serve the same role; any single one is sufficient.

**Alive-beacons — `NamespaceStateHeartbeat` promoted from informational.** Each current member periodically signs and gossips `{group_id, state_hash, governance_dag_heads, hlc}`. P listens for a short bootstrap window and sees what state hash multiple distinct members agree on. Wire-format infrastructure already exists (§3.5); the promotion is from "debug-log only" to "load-bearing input to bootstrap convergence detection."

**Multi-source bootstrap fallback.** If Owner/TEE is offline and unreachable, fall back to dialing ≥2 distinct peers and taking the longest valid governance DAG. X can only fully eclipse P by controlling **all** of P's bootstrap candidates — eclipse is then a network-layer assumption, not a Calimero-layer guarantee. Configurable per-deployment whether to block bootstrap on Owner/TEE unreachability (strict) or fall back to multi-source (light-touch).

**Owner-transfer chain verification.** If the role model permits Owner-transfer, P's bootstrap-known Owner peer-id may be stale. P fetches the governance DAG from the supposedly-current Owner, verifies the OwnerTransfer chain back to genesis (each transfer signed by the previous Owner), and trusts the latest Owner the chain resolves to.

---

## 7. Recovery

### 7.1 Owner-signed snapshots

Periodic snapshots of group state, **signed by Owner** (not by quorum). Cap rebuild scope when a peer needs to recover from wrongly-applied deltas (e.g. a bug regression, not a Byzantine attack). Used in two ways:

1. **Bounded rebuild.** A peer that detects taint rebuilds from the most recent snapshot + valid deltas only, instead of from genesis.
2. **Bootstrap floor.** Deltas claiming `governance_position` older than the most recent sealed snapshot are rejected outright. (This is the weak-subjectivity-checkpoint pattern from PoS chains.)

Snapshot frequency and trigger are deployment-configurable. Reasonable defaults: every N governance ops or every T seconds, whichever comes first; Owner triggers manually for high-stakes deployments.

### 7.2 Local rebuild tool

Detect tainted state (deltas with rejected `governance_position`), re-derive from most recent valid snapshot + replay only valid deltas. Operator-invoked; not part of automatic recovery in v1.

---

## 8. Edge cases

### 8.1 Concurrent governance ops across partitions

Two admins on disjoint partitions concurrently propose conflicting ops (one removes X, another promotes X to admin). Each propagates; receivers see both at concurrent governance-DAG positions. With quorum off the table, resolution is purely **HLC tiebreak** — last-writer-wins on membership state, with the loser becoming a no-op. Calimero's existing `SignedGroupOp.current_state_hash` divergence check (`compute_group_state_hash` embedded in the op for #2284) already provides fork *detection*; HLC tiebreak is the resolution policy.

For the case where the wrong admin wins the tiebreak, recovery is via §5.7 Owner override — Owner can sign the corrective op.

### 8.2 Honest-claimed-old `governance_position`

Byzantine X signs a delta claiming `governance_position = H_old` despite having actually observed `H_remove`. There is no way to disprove the negative observation in async consensus (§6.1). With universal D1, this attack is bounded by gossip-propagation time of `MemberRemoved` — X has at most that window to slip messages through before every honest peer drops them at the libp2p layer. After the window, X is fully cut off.

The defense is **detection plus bounding**, not prevention: §6.2 D1 bounds the time window, §6.3 D2 / D3 add belt-and-suspenders, and forensic tooling can flag inconsistencies if X is sloppy enough to also ack newer governance ops (which would contradict their claim of "I hadn't observed `MemberRemoved` when I signed").

### 8.3 Authored-but-unobserved-cut

Group {A, B, C}. A removes B; B has shared state with C that A doesn't have; A proposes the removal. Under §5:

- B's writes that went to C carry `governance_position = H_old` (some pre-removal state).
- C eventually propagates them to A.
- A validates: `H_old` does not descend from `MemberRemovedOp.cut` in A's governance DAG → valid → applies.
- Final state on A and C: identical.

The reason there's no divergence is that the validity rule is stable across peers (a function of `governance_position` and the embedded cut, both of which all peers eventually agree on), rather than a function of "what the proposer happened to observe at proposal time."

---

## 9. Implementation plan — categorized issues

Five categories. Sizes: **S** = ~1 week, **M** = 2–4 weeks, **L** = multi-month.

### A. Observability & convergence detection

| # | Issue | Size | Depends on |
|---|---|---|---|
| **A1** | Expose `state_hash` on `GET /admin-api/groups/:group_id` by wiring `compute_group_state_hash` through `GroupInfoApiResponseData` (`crates/server/primitives/src/admin/mod.rs:84`) | S | — |
| **A2** | Add `wait_for_governance_sync` workflow step in merobox; polls `state_hash` across nodes (mirrors `WaitForSyncStep`) | M | A1 + release cascade |
| **A3** | Hierarchical Merkle: `NamespaceMerkle { meta, members, governance_dag_root, subgroups, contexts }` exposed via API for whole-subtree convergence checks | L | A1, B1, C1 |

### B. Cross-DAG causal authorization

The foundational change: state deltas reference governance state, receivers enforce it.

| # | Issue | Size | Depends on |
|---|---|---|---|
| **B1** | Add `governance_position` field to `ContextDagDelta` / `CausalDelta`. Wire-format breaking change. Replace today's empty-vec `governance_epoch`. | M | — |
| **B2** | Receiver-side: buffer state deltas whose `governance_position` references governance state not yet observed locally; release once governance catches up | M | B1 |
| **B3** | Apply-time check: `delta.signer` must have been a member at `delta.governance_position`. Replaces the partial `is_read_only_for_context` check; covers `User`-storage actions too. | M | B1, B2 |

### C. Removal semantics & recovery

| # | Issue | Size | Depends on |
|---|---|---|---|
| **C1** | Admin-signed deterministic cut on `MemberRemoved` — embed `cut: GovernancePosition` in the signed op | M | B1 |
| **C2** | Self-signed `MemberLeft` cut — leaver embeds their own `governance_position` at sign time | S | B1 |
| **C3** | Owner override / `MemberRestored` — Owner can sign a reversal of an admin's `MemberRemoved` | M | C1 |
| **C4** | Forward-only semantic — declare and enforce: writes from a pre-cut `governance_position` are honored regardless of arrival order | S | B3, C1 |
| **C5** | Owner-signed snapshot mechanism — periodic checkpoints of group state for bounded rebuild | L | C1 |
| **C6** | Local rebuild tool — operator-invoked, replays from most recent snapshot using valid deltas only | M | C5, B3 |

### D. DoS & Byzantine defenses

| # | Issue | Size | Depends on |
|---|---|---|---|
| **D1** | Network-layer deny-list keyed on **signer identity** (not libp2p peer-id) — drops gossip from removed members at the libp2p apply boundary. **Load-bearing defense.** | S | — |
| **D2** | Time-window validity: `delta.hlc < governance_position.applied_at + W`. Belt-and-suspenders for D1. | S | B1 |
| **D3** | Old-key (K0) deprecation policy after `MemberRemoved` + grace period | S | C1 |
| **D4** | Per-peer rate limits at gossip apply layer — only if empirical evidence of slow-leak attacks emerges | M | — |

### E. Bootstrap & eclipse resistance

| # | Issue | Size | Depends on |
|---|---|---|---|
| **E1** | Owner / TEE peer-id-authenticated bootstrap endpoint — late joiner dials by peer-id, receives current member set + state hash + governance DAG heads | M | A1 |
| **E2** | Promote `NamespaceStateHeartbeat` from informational logging to load-bearing alive-beacon — multi-member state-hash agreement during bootstrap window | S | A1 |
| **E3** | Multi-source bootstrap fallback for when Owner/TEE is unreachable — connect to ≥2 peers, take longest valid governance DAG | M | E1 |
| **E4** | Owner-transfer chain verification — late joiner discovers current Owner via signed transfer chain back to genesis | S | E1 |

---

## 10. Recommended landing order

### Phase 1 — Independent quick wins (parallel, ~1–2 weeks each)

1. **A1** — exposes governance state hash, immediately useful for testing
2. **A2** — workflow step type; follows A1 release cascade
3. **D1** — network deny-list on removal; **the load-bearing long-range defense**
4. **E2** — alive-beacon promotion of `NamespaceStateHeartbeat`

### Phase 2 — Foundational cross-DAG primitives (sequential, ~1–2 months)

5. **B1** — `governance_position` wire format change; breaking, coordinate with release
6. **B2 + B3** — buffering + apply-time validation
7. **C4** — declare and enforce forward-only semantic now that the primitives exist

### Phase 3 — Removal flow + recovery (~1–2 months)

8. **C1** — admin-signed deterministic cut on `MemberRemoved`
9. **C2** — self-signed `MemberLeft` cut
10. **C3** — Owner override / `MemberRestored`
11. **D2 + D3** — time-window + K0 deprecation, layered defense
12. **C5 + C6** — Owner-signed snapshots + rebuild tool

### Phase 4 — Bootstrap & long-tail

13. **E1 + E3 + E4** — Owner/TEE bootstrap, multi-source fallback, transfer-chain verification
14. **A3** — hierarchical Merkle for whole-subtree convergence
15. **D4** — rate limits, only if empirically motivated

---

## 11. Summary — what Calimero today does NOT do

1. **Governance state hash is not exposed on the admin API.** Tests use `type: wait, seconds: N` because there is no equivalent of `rootHash` for `get_group_info`.
2. **State deltas do not reference the governance state they were authorized against.** The `governance_epoch` field exists in the wire format but is populated as `vec![]` and ignored on receive since #2237 Phase 11.2.
3. **Receiver-side membership check on state deltas is partial.** Only `ReadOnly` / `ReadOnlyTee` are rejected; outright removal returns `None` from `get_group_member_role`, treated as "pass through."
4. **For `User`-storage actions there is no membership check at all** — only Ed25519 signature + nonce replay protection.
5. **The `Shared`-storage causal writer-set check is not automatically connected to `MemberRemoved`** — rotation log entries come from explicit rotation actions, not from governance ops.
6. **No deterministic cut declaration.** The cut, where it exists conceptually, is each peer's local apply-time of the op — different on every peer.
7. **No network-layer deny-list on removal.** A removed member's gossipsub messages continue to be processed.
8. **No time-window validity** on signatures from removed members.
9. **No Owner-signed snapshot mechanism** to bound rebuild scope or seal the governance history against long-range claims.
10. **No authoritative bootstrap path.** A late-joining peer can be eclipsed by a single malicious bootstrap source.
11. **`NamespaceStateHeartbeat` is informational only** — does not contribute to bootstrap or convergence detection.

What *does* bound divergence today:

- **Key rotation on `MemberRemoved`** — closes interactive protocols. Does not stop one-way writes encrypted with K0 during the apply-lag / partition window.
- **Eventually-detected `root_hash` divergence** — operators can grep `NamespaceStateHeartbeat` debug logs. No automatic remediation.
- **`SignedGroupOp.current_state_hash` divergence check** — rejects governance ops signed against a stale group state. Internal to the governance DAG; does not touch state.

---

## 12. Related work in the codebase

- **#2237 governance broadcast core** — `publish_and_await_ack` + `AckRouter` + readiness FSM. Infrastructure that admin-signed deterministic cuts (C1) build on.
- **#2266 DAG-causal Shared verifier** — `writers_at(parents)` + rotation log. The receiver-side causal authorization model for Shared CRDTs. B3 generalizes this to `User`-storage and to all writers (not just rotation actions). [ADR-0001](../adr/0001-shared-storage-concurrent-rotation.md) is the formal spec.
- **#2284 state hash includes owner_identity** — `compute_group_state_hash` covers members + roles + admin + owner + app. The hash A1 would expose is the same primitive.
- **[Membership & Leave](../../architecture/membership-and-leave.html)** — formalizes the Owner role and self-leave operations. C1 / C2 / C3 build directly on this role model.

---

## 13. Open questions

1. **Wire-format breaking change strategy.** B1 changes the on-wire shape of `ContextDagDelta`. Versioned protocol message? Hard cut at a release boundary?
2. **Owner mandatory in role model?** C3 (Owner override) and E1 (Owner-anchored bootstrap) both assume Owner is always present. If the role model permits Owner-less groups, both need a defined fallback. Easiest answer: make Owner mandatory in v1.
3. **Owner-transfer semantics.** Does the role model permit Owner transfer? If yes, what's signed by whom? E4 needs the answer.
4. **Snapshot frequency and trigger.** For C5: periodic (every N ops, every T seconds) or admin-triggered? Per-namespace configurable?
5. **Time-window W.** D2 needs a concrete value. 24h working default; per-namespace configurable?
6. **Network deny-list scope.** D1 — per-group, per-namespace, or global per-signer-identity? What if the same signer is a member of group X (where they were removed) and group Y (where they're still active)?
7. **Bootstrap unreachability policy.** E1 — strict (block bootstrap if Owner/TEE unreachable) or fall back to E3 multi-source? Per-deployment configurable, but what's the default?
8. **Epic vs separate issues.** The 22 issues here decompose into 4 phases. Tracked as one umbrella epic with sub-issues, four separate epics, or twenty-two standalone?

---

## 14. Out of scope (intentionally not addressed here)

- **Quorum / M-of-N voting / multisig-style approvals.** See §1 and §5.1. Off the table by design.
- **Token-economic incentives** for voting / quorum participation — Calimero is not a public chain.
- **BFT-style consensus on every governance op** — would defeat the point of CRDT-style permissionless writes within the member set.
- **Full retroactive invalidation of removed-member contributions to CRDT state** — fundamentally hard in async CRDT-with-access-control settings; the "forward-only" semantic in §5.6 is the realistic stance.
- **Hardware-backed identity** (TEE-anchored signatures beyond what `ReadOnlyTee` already provides).
