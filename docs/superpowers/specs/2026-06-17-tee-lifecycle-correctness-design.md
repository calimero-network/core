# TEE membership-lifecycle correctness — #2770 / #2726 / #2771 (shared design)

| | |
|---|---|
| **Issues** | core #2770 (emit-before-persist race), #2726 (subgroup purge-failure reconcile), #2771 (redundant transient ReadOnlyTee row in flipped-to-Open subgroups) |
| **Scope** | Three sequenced PRs in the governance-store self-purge + apply subsystem. One shared design; one PR each. |
| **Date** | 2026-06-17 |
| **Lineage** | Builds on the merged self-purge namespace sweep (#2721/#2725/#2724/#2764), Phase 1 admission (#2772, open), and group-key deletion (#2776, open/merge-ready). |

---

## Sequencing & dependencies

```
#2770 (PR-1) ──────────────── independent (apply-path: group + namespace)
#2776 (open, merge-ready) ──▶ #2726 (PR-2)   ← only hard dep; runs in parallel with PR-1
#2772 (open, merge-ready) ──▶ #2771 (PR-3)
```

- **PR-1 (#2770)** is independent (governance-store apply path); it removes a pre-existing race and can land any time.
- **PR-2 (#2726)** hard-depends **only on #2776** (its residue check looks for the AES `GroupKeyEntry` rows #2776's purge deletes). It does **not** hard-depend on PR-1 — research confirmed the "delete op-log while apply still writing it" window is **pre-existing and orthogonal** (the namespace purge path already lives with it), so PR-2 inherits whatever apply ordering the base provides. **PR-1 and PR-2 can proceed in parallel.**
- **PR-3 (#2771)** depends on **#2772** (the `tee_subgroup_admit` subscriber + the create/visibility surface it builds on).

---

## PR-1 — #2770: emit OpEvents after the op-log entry persists

**Problem.** In the **group-op** apply path (`apply_group_op_mutations`, `lib.rs:~1271` / `governance.rs:~1275`), per-op handlers call `op_events::notify` *inside* dispatch — before the op-log entry is persisted (`lib.rs:~1339` / `governance.rs:~1365`). There is no transaction, and writes are write-through. A subscriber on another task can react to the event and read the op-log back before it's written. This forced the bounded wake-then-reread in `tee_subgroup_admit` (#2772) and opens a "self_purge deletes the op-log while apply is still writing it" window.

**Decision (settled): emit-on-append-only.** Today the mutation re-runs (and re-emits) on idempotent replay. Research confirmed **no subscriber relies on replay re-emission** — `auto_follow`, `self_purge`, `tee_subgroup_admit` are all idempotent and tolerate dropped events (the broadcast channel is lossy under lag). So we emit **once, on first op-log append**, not on replay/dedup. Strictly more correct.

**Mechanism (Option A — collect-then-flush):**
- Add `pending_events: Vec<OpEvent>` to `GroupApplyCtx` (`ops/group/context.rs`), mirroring the existing `pub(crate) divergence` field; add a `queue_event` helper.
- The **8 group emit sites** push into the sink instead of calling `notify`: `member_added.rs` (`MemberAdded` + the `emit_auto_follow_set_if_enabled` helper `AutoFollowSet`), `member_joined_via_tee_attestation.rs` (`TeeMemberAdmitted` + `AutoFollowSet`), `member_removed.rs` (`MemberRemoved`/`TeeMemberRemoved`), `member_left.rs` (`MemberRemoved`/`TeeMemberRemoved`, cascade + self), `member_set_auto_follow.rs` (`AutoFollowSet`), `context_registered.rs` (`ContextRegistered`).
- `apply_group_op_mutations` returns the collected events (extend the existing `divergence` return).
- **Drain + notify after persist, on the append branch only** — both flush sites: `lib.rs` after `persist_group_governance_progress` (inside the append path, not the content-hash dedup early-return) and `governance.rs` after `persist_group_op_log_entry` (inside `if handled && !already_logged`). Gating on the append achieves emit-once.

**Must NOT move:** `GroupKeyDelivered` (`governance.rs:~1029`, the `apply_received_group_key` path) — it's not a per-op-dispatch emit, has no op-log append to gate on, and its `join_group` wake-then-reread is latency-sensitive. Leave it exactly as a direct `notify`.

**Namespace path — IN SCOPE (confirmed racy).** Research traced `apply_signed_op`: the RootOp emit happens in `dispatch_root_op` (`governance.rs:240`) but the op-log entry persists later at `store_operation` (`governance.rs:425`) — same emit-before-persist race as the group path. So PR-1 must **also** thread a sink through `NamespaceApplyCtx` (currently sink-less) + `dispatch_root_op`, and drain after `store_operation`. **Three namespace emit sites:** `SubgroupCreated` (`ops/namespace/group_created.rs:121`), `SubgroupReparented` (`ops/namespace/group_reparented.rs:22`), and `MemberJoined`/`MemberJoinedOpen` → `emit_auto_follow_set_if_enabled` (`namespace/membership.rs:71`). The RootOp path's own dedup is *before* emit (`governance.rs:227`), so it doesn't double-emit on replay — only the persist-ordering needs fixing.

**Replay semantics (observable, document it):** append-gated drain also stops re-emitting on duplicate/backfill/retry of an already-logged op (today it re-emits). No *first* apply loses its event (every emit-worthy op also appends on first apply); the change only removes spurious replay re-emits — a net improvement worth a changelog note.

**Follow-up (not in PR-1):** once both PR-1 and #2772 are merged, remove the now-redundant bounded wake-then-reread in `tee_subgroup_admit::handle_new_tee_member` (fold into PR-2 or a tiny standalone cleanup). Add a doc note on `GroupApplyCtx.pending_events`: future group handlers must `queue_event`, never `notify` directly, or they reintroduce the race.

**Tests:** a subscriber that reads the op-log on its triggering event observes the entry (race gone); a replayed op does not double-emit; existing governance-store + the three subscribers' tests stay green.

---

## PR-2 — #2726: subgroup purge-failure reconcile

**Problem.** A subgroup-only self-purge whose load-bearing delete partially fails leaves private key residue with no recovery: the `PurgeAction::Subgroup` path writes no marker, `purge_subgroup_for_self` returns no `Result`, and the startup `reconcile_sweep` (which only handles namespace-root markers) never retries it. The one-level-down mirror of the gap #2721 closed.

**Three steps:**

1. **`Result`-refactor `purge_subgroup_for_self`** (`self_purge.rs:759`): return `eyre::Result<()>`, `Err` **iff** the load-bearing `delete_group_local_rows` step (`self_purge.rs:800`) fails; best-effort context/tree-edge failures stay non-fatal (mirror the namespace two-class split). The caller marks-before / clears-on-`Ok` / leaves-on-`Err`.

2. **Marker storage A1** (confirmed over A2): a **new `calimero-store` key prefix + `PendingSubgroupPurge` key type + `PendingSubgroupPurgeRepository`**, each a 1:1 copy of `PendingSelfPurge`/`PendingSelfPurgeRepository`. Rationale: the marker key is `[prefix][32-byte id]` with a `()` value, so reusing the namespace marker type (A2) makes subgroup markers **byte-indistinguishable** from namespace markers — and disambiguating by re-resolving the namespace is impossible exactly in the failure case (the subgroup's tree edges are already deleted). A1 gives the sweep a disjoint, unambiguous enumerator. Pick the next free prefix byte (verify against the `*_PREFIX` list; `0x3F` appears free).
   - **Mark before purge** on the `PurgeAction::Subgroup` arm (`self_purge.rs:694`, symmetric to the namespace arm at `:707`); clear on `Ok`, leave on `Err`.

3. **Extend `reconcile_sweep`** with a **second loop** over the new marker repo, **run after the existing namespace pass** (namespace-first: any subgroup under a just-namespace-purged subtree then hits the already-purged predicate and clears its stale marker for free). No marker-precedence logic is needed — research confirmed a namespace-root TEE eviction emits a *single* root `TeeMemberRemoved` (the `cascade_remove_member_from_group_tree` cascade is silent / ContextIdentity-only), so concurrent namespace+subgroup markers for one node don't arise from a root eviction. The subgroup pass applies a **3-state predicate** (checking the **subgroup `gid`**, not root — and it cannot reuse the namespace "identity-gone" signal since identity is namespace-scoped):
   - **healthy** — `MembershipRepository::role_of(gid, self_pk).is_some()` (direct role) → clear marker, don't purge.
   - **evicted-with-residue** — no role **and** residue present → re-run `purge_subgroup_for_self` (idempotent).
   - **already-purged** — no role **and** no residue → clear marker, don't purge.
   - **Residue = dual-family (the #2776 interaction):** `GroupKeyring::load_current_key_record(gid).is_some()` (AES `GroupKeyEntry`) **OR** `SigningKeysRepository::get_key(gid, self_pk).is_some()` (signing key). #2776 made `delete_group_local_rows` delete both, non-atomically with a first-error return, so a partial failure can leave "signing gone / AES present" — checking only signing keys would miss it.
   - Reuse `PurgeBranch::Subgroup` + extend `ReconcileOutcome` for metrics.

**Startup-only** sweep (periodic is a deferred follow-up). **Effort ~5–7 dev-days.** `purge_subgroup_for_self` has only 2 callers (the live arm + one test) and the namespace cascade uses a different path, so the `Result`-refactor is isolated. Group-key deletion runs **only** via the public `delete_group_local_rows` (the `delete_all_for_group` helpers are `pub(crate)`); the residue *probes* (`load_current_key_record`, `get_key`) are public, so reading residue from `calimero-context` is fine.

**Tests:** mark-before-purge symmetry; `Result` drives clear/leave; the 3-state predicate (residue-present → purge; already-purged → clear; healthy → clear), with residue covering both key families incl. the signing-gone/AES-present case; namespace-first ordering clears a subgroup marker under a purged namespace; no false-purge of a re-admitted subgroup member. **Test caveat:** `InMemoryDB` deletes always succeed, so we can't inject a real `delete_group_local_rows` failure — follow the existing namespace tests' pattern (unit-test the pure decision functions + hand-seed the post-failure "marker + residue present" store state + drive the happy-path cascade on a clean store), not a fault-injecting `Store` mock. Match #2725 coverage.

---

## PR-3 — #2771: eliminate the redundant transient admission (structural fix)

**Problem (real & structural, not cosmetic-only).** There is **no atomic create-Open path** — `CreateGroupRequest` carries no visibility, so every Open subgroup is born **Restricted** (default) then flipped via a separate `SubgroupVisibilitySet` op. In the window between create and flip, the `tee_subgroup_admit` create-time subscriber **reliably** admits the entitled TEE node into the (momentarily Restricted) subgroup — writing a direct `ReadOnlyTee` row **and delivering the per-subgroup key**. After the flip to Open, the node would have read the subgroup via inheritance + the namespace key anyway, so the admission + key delivery are **redundant**, on **every** Open-subgroup creation.

**Why not prune after the flip (rejected).** A silent local delete of the redundant row **breaks convergence**: `hash_group_state` hashes every `GroupMember` row, so a local-only delete diverges the subgroup state hash across peers → permanent governance desync. An op-based prune is disproportionate (a new replicated op in a security-sensitive path that would emit `TeeMemberRemoved` → trigger `self_purge` → strip the keys, the opposite of intent).

**Fix (structural): born-Open create — Option C (no wire-format break).** Add an optional `visibility` (default Restricted) to the create request, and when Open is requested, have the create-group **handler** establish Open *before* the create's `SubgroupCreated` event reaches the subscriber — by emitting the existing replicated **`SubgroupVisibilitySet { Open }` op** as part of the create flow (Option C), rather than adding a `visibility` field to the borsh-serialized `RootOp::GroupCreated` (Option A — rejected: it's a replicated-op wire-format break that changes op-log hashes, breaks un-upgraded peers, and touches `op-adapter`/`governance-types`).

Why C is sufficient: the only node that does the redundant admission is the **creator** (it runs `tee_subgroup_admit` + holds the keys). C closes the window on the creator (visibility row Open before the subscriber's `is_open_chain_to_namespace` check), so it never self-admits; remote peers converge via the replicated visibility op and never self-admit anyway. **The make-or-break:** the Open visibility row must be committed **before** `notify_op_event(SubgroupCreated)` so the subscriber sees Open and skips.

**Keep key minting unconditional** — do NOT skip the per-subgroup key mint for born-Open: it's load-bearing for the subscriber's key-holder check *and* for a later flip-to-Restricted. (So PR-3 adds no conditional-minting logic — simpler.)

**Scope:** the wire `CreateGroupApiRequest` (`server/primitives`, backward-compatible optional field) **and** the namespace-subgroup path `CreateGroupInNamespaceBody` (the path meroctl actually uses) both need the field for end-to-end born-Open; plus the internal `CreateGroupRequest` (free to extend) and ~5 constructor sites (all default to Restricted). No change to the `SubgroupVisibilitySet` apply itself. No published-contract / mero-tee impact.

**Tests:** a subgroup created with `visibility: Open` is Open immediately (no flip); `tee_subgroup_admit` does NOT admit the TEE node into it (no redundant row, no key delivery); default (no visibility) stays Restricted (unchanged behavior); existing create-then-flip still works.

---

## Non-goals / out of scope

- Periodic reconcile sweep (PR-2 is startup-only; periodic is a later follow-up).
- Adding `visibility` to the borsh `RootOp::GroupCreated` (PR-3 Option A — rejected; use the second-op approach to avoid a replicated-op wire break).
- Any change to `GroupKeyDelivered` semantics (stays a direct, pre-persist-safe notify).
- mdma / mero-tee — these are core-only changes; no contract-crate (`calimero-server-primitives`, `calimero-tee-attestation`) public-surface change, so no mero-tee rev bump.
