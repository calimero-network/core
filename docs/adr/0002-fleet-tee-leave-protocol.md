# ADR 0002 — Fleet TEE leave hygiene (post-eviction cleanup)

| | |
|---|---|
| **Status** | Accepted — implemented (see §Implementation as shipped) |
| **Date** | 2026-06-02 (proposed); 2026-06-08 (updated to match implementation) |
| **Deciders** | Calimero core team, mero-tee KMS team |
| **Context** | [mdma#155](https://github.com/calimero-network/mdma/issues/155) — HA disable doesn't propagate to merod; `[[project-ha-no-convergence-loop]]` — parent reconcile-loop gap |
| **Constrains** | Local soft-vs-hard cleanup choice for evicted members; the self-purge listener + reconcile in `crates/context/src/self_purge.rs` |
| **Implemented by** | [#2680](https://github.com/calimero-network/core/pull/2680) (role-scoped purge), [#2724](https://github.com/calimero-network/core/pull/2724) (failure-class gating + metric), [#2725](https://github.com/calimero-network/core/pull/2725) (marker-based reconcile) |

> Earlier drafts of this ADR scoped a full pull-based leave protocol (sidecar polls steady-state, fleet-leave path, KMS forget). That over-reached: `MemberRemoved` *already* does cryptographic eviction via the existing key-rotation pipeline. The user-visible "I disabled HA but still see the fleet node" complaint is solved client-side by tauri-app issuing `remove_group_members` on disable. What remains is the *hygiene* tail: local data purge on the evicted node, stale sidecar cache, and the broader sidecar-reconcile-loop story.
>
> **Update (2026-06-08):** the original draft concluded "core: NO CHANGE — defer the local hard purge." That was revisited during implementation. Forward secrecy is provided by key rotation regardless, but a **`ReadOnlyTee`** member has no rejoin pathway (re-admission derives a *fresh* attestation pubkey), so leaving its on-disk signing-key material around buys nothing and is a forward-secrecy hygiene risk. So core *does* now hard-purge — but **only for the TEE role**, leaving the soft-leave path intact for everyone else. §Decision(a) and §Implementation record what shipped.

## Context

When the owner publishes `MemberRemoved` for a `ReadOnlyTee` member (the path that fires when the desktop client calls `remove_group_members` after a successful cloud `disable-ha`), the existing key-rotation pipeline in `calimero-governance-store` already handles cryptographic eviction: it generates a fresh group key wrapped for everyone **except** the removed member, so the evicted node can decrypt nothing written after eviction.

Effects on the evicted fleet node, *before* this ADR's work:

- **Membership row removed** on the fleet node's merod when it applies the gossiped op; entry added to the deny-list to prevent state-delta replay-in.
- **Forward secrecy on new writes**: the fleet node never receives the rotated key. Cannot decrypt anything written after eviction.
- **Local key material + identity on the fleet's disk**: *untouched* under the old "soft leave" default. The node's keyring still held the old group key and its `NamespaceIdentity`.

The open hygiene tail, narrowed to what was actually missing:

1. **Soft-vs-hard local cleanup on the evicted node.** Historically "soft" (rows removed, keys retained). For a regular member this is *desirable* — it's what `kick-and-rejoin-keyshare` / inheritance-rejoin reuse. For a `ReadOnlyTee` node — which cannot rejoin under the same identity — retained key material is pure hygiene debt.
2. **Fleet sidecar reconcile loop** (mero-tee). Cosmetic-but-real: the sidecar's cache going stale for namespaces it's no longer in. Composes with the broader `[[project-ha-no-convergence-loop]]` rework.
3. **KMS forgetting** (mero-tee). Whether the KMS should release attestation-bound key material on eviction. Cross-team flag, independent of core.

## Decision

**Treat eviction hygiene as independent follow-ups, each scoped narrowly, none blocking the user-visible feature.**

### (a) Local cleanup — role-scoped hard purge (implemented)

The soft-vs-hard choice is resolved **by role**, not globally:

- **Non-TEE removals stay soft.** `Admin` / `Member` / `Observer` removals keep their local rows (identity, signing keys, contexts) so `kick-and-readd` / `rejoin-via-keyshare` / inheritance-rejoin can reuse them. Hard-purging these would regress the `apps/scaffolding-e2e/workflows/group-{kick,leave}-*` workflows. This is the soft-leave invariant.
- **`ReadOnlyTee` removals hard-purge.** A TEE node admitted via `MemberJoinedViaTeeAttestation` re-derives its identity from a *fresh* attestation on any future admission, so there is no rejoin path that could reuse the old material. Leaving it on disk buys nothing and is forward-secrecy hygiene debt. So a TEE eviction purges the local signing-key material, `NamespaceIdentity`, gov-op log, and (for a namespace-root eviction) unsubscribes from the namespace gossipsub topic.

This is **not** a new forward-secrecy mechanism — FS on future writes is provided by the key-rotation pipeline, independent of the purge. The purge deletes the now-orphaned old material the rotation already made useless.

### (b) Fleet sidecar reconcile — small, in mero-tee

Sidecar drops stale cache entries when a namespace disappears from `/should-join`. No protocol or merod changes. Sidecar-internal, in `mero-tee`. (Tracked/landed separately; not core's scope.)

### (c) KMS forget — cross-team flag

On eviction the fleet node's KMS could invalidate the attestation-bound key it holds for the namespace. Requires a KMS-side invalidation API, a trigger path, and a clear threat model (the TEE could have copied plaintext while admitted). Filed as a cross-team flag, owned by the mero-tee team. Not a core decision.

## Implementation (as shipped)

All in `crates/context/src/self_purge.rs` unless noted. The handler is a detached listener spawned per node, mirroring the `auto_follow` architectural split (self-detection — "did this op evict *me*?" — is per-node state, not part of the node-agnostic apply contract).

### Role-scoped event listener (#2680)

The listener gates on `OpEvent::TeeMemberRemoved`, **not** the generic `OpEvent::MemberRemoved`. Both are emitted by the apply path; `TeeMemberRemoved` fires only when the removed member's stored role was `ReadOnlyTee`. This is what keeps the soft-leave path intact for non-TEE removals (§Decision(a)).

- **Subgroup-only eviction** → purge that subgroup's local rows; keep the `NamespaceIdentity` + gossipsub subscription (other memberships under the namespace still need them).
- **Namespace-root eviction** → cascade-purge the whole subtree, then drop namespace-level state, then unsubscribe.

### Failure-class gating (#2724)

The namespace cascade tracks two failure classes:

- `signing_key_purge_failed` — the security-critical `delete_group_local_rows` (signing-key material) failed. Load-bearing.
- `context_cleanup_failed` — a best-effort dead-pointer cleanup (context-index unregister, tree-edge delete) failed. Non-security.

Dropping the `NamespaceIdentity` and unsubscribing are gated on the **signing-key purge only**. A best-effort cleanup failure no longer blocks the security-critical finalize. A `self_purge_failures_total` Prometheus counter (labeled by branch × class) records every failure (#2686).

### Marker-based reconcile (#2725)

`TeeMemberRemoved` fires once per eviction, and an already-evicted identity receives no further events — so a *missed* or *partially-failed* purge has no event-driven retry. Recovery is a **startup reconcile**, made role-safe by a durable **pending-self-purge marker** (`PendingSelfPurge` store key):

- The marker is written **only** in the listener's namespace-purge dispatch — reachable only for a confirmed `TeeMemberRemoved` matching this node's identity (node-aware *and* role-aware). Cleared on full purge success.
- On startup the reconcile enumerates **markers only** and purges a namespace **iff (marker present) AND (still no surviving membership)**. Identity-gone or re-admitted clears the stale marker without purging; a read error skips (never purge on uncertainty).

The marker is essential: post-eviction the role row is erased, so a role-blind scan of `NamespaceIdentity` rows could not distinguish evicted-TEE residue from a *pending join* (identity written before the membership row materializes) or *non-TEE soft-leave residue* (identical shape, but must be kept for rejoin) — and would false-purge both. The marker is the role/intent gate; the still-evicted check is the safety gate; both must hold.

## Alternatives considered

### Full pull-based leave protocol (the original ADR draft)

Redefine `/should-join` to set-state, add a `fleet_leave` path in core, wire the sidecar to issue `meroctl tee fleet-leave`. **Rejected**: `fleet_leave` would invoke `leave_group` self-leave, whose key rotation is the deferred two-phase design — *worse* forward secrecy than the owner-side `MemberRemoved` path, which already rotates correctly. A fleet-initiated leave would be redundant or actively wrong.

### Role-blind reconcile scan (first cut of #2725)

Scan every `NamespaceIdentity` and purge any with no surviving membership. **Rejected** in review: false-purges pending joins and non-TEE soft-leave residue (see §Implementation → marker-based reconcile). Replaced by the marker gate.

### Cloud-to-fleet push notification

mdma webhook to fleet nodes on disable. **Rejected**: server surface on fleet sidecars, addressability on mdma, solves nothing the tauri-driven `remove_group_members` doesn't.

## Consequences

### What we lock in

- The user-visible HA-disable flow stays client-driven: tauri-app calls `remove_group_members` on the owner's local merod after a successful cloud disable. (Separate tauri-app issue.)
- **Role-scoped cleanup**: soft-leave for non-TEE (rejoin material retained), hard-purge for `ReadOnlyTee` (no rejoin path).
- Recovery for missed/partial TEE purges is a **marker-gated startup reconcile**, not an event retry.
- Fleet sidecar reconcile + KMS forget remain mero-tee-owned.

### What we deliberately DON'T do

- Add a `fleet_leave` path in core. Owner-side `remove_group_members` is the correct eviction primitive.
- Hard-purge non-TEE removals. Would regress the soft-leave rejoin workflows.
- A continuous/periodic reconcile. Startup-only for now (see §Failure modes).

### Failure modes we accept

- **Historical-blob retention on an evicted TEE before purge / on the soft-leave path.** No forward-secrecy delta — FS is provided by key rotation, not the purge. The TEE hard-purge deletes the node's *own* now-orphaned key material as hygiene.
- **Pure lagged-drop of a `TeeMemberRemoved` event.** If the broadcast channel lags (requires >1024 events between recv calls — rare) the listener never dispatches, so no marker is written and the reconcile cannot recover it. Residue persists until a future eviction event or a periodic reconcile (not yet built). Bounded and rare; not an FS hole.
- **Partial cascade failure (`signing_key_purge_failed`).** The `NamespaceIdentity` + signing-key residue is left on disk *with the marker*, and the startup reconcile completes it on the next restart. No event-driven retry within a session.
- **Subgroup-only purge-failure residue.** The reconcile is namespace-level: a node kicked from a single subgroup (still a namespace member) whose `purge_subgroup_for_self` partially failed keeps subgroup-level residue the sweep won't catch (its still-evicted gate correctly keeps a live namespace member). The **subgroup-reconcile follow-up** is tracked in [#2726](https://github.com/calimero-network/core/issues/2726).
- **Stale sidecar cache for ~1 poll cycle.** Bounded by the `should-join` poll interval (mero-tee).
- **KMS holding unused key material** until KMS-side forget lands. Acceptable given the threat model.

## Open questions

1. **Periodic reconcile** — startup-only leaves a mid-session lagged-drop uncovered until the next restart. Worth a periodic pass if that window matters.
2. **Subgroup-level reconcile** — [#2726](https://github.com/calimero-network/core/issues/2726); pairs with giving `purge_subgroup_for_self` a `Result`/retry surface ([#2692](https://github.com/calimero-network/core/issues/2692)).
3. **KMS forget semantics** — owned by the mero-tee team.

## Related

- `[[project-ha-no-convergence-loop]]` — parent architectural gap (sidecar reconcile-loop story).
- `[[project-ha-fleet-join-working-recipe]]` — sister recipe; leave hygiene must compose without regressing join.
- [#2680](https://github.com/calimero-network/core/pull/2680), [#2724](https://github.com/calimero-network/core/pull/2724), [#2725](https://github.com/calimero-network/core/pull/2725) — the implementing PRs.
- [#2686](https://github.com/calimero-network/core/issues/2686), [#2692](https://github.com/calimero-network/core/issues/2692), [#2721](https://github.com/calimero-network/core/issues/2721), [#2726](https://github.com/calimero-network/core/issues/2726) — tracked follow-ups.
- mdma#155 — the issue that surfaced the gap.
- `crates/context/src/self_purge.rs` — the listener, cascade, failure-class gating, and reconcile.
