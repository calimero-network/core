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
#2770 (PR-1)  ─────────────┐
#2776 (open, merge-ready) ──┴──▶ #2726 (PR-2)
#2772 (open, merge-ready) ──────▶ #2771 (PR-3)
```

- **PR-1 (#2770)** is independent (governance-store apply path) — lands first; it removes a race class the others would otherwise have to work around.
- **PR-2 (#2726)** depends on **both** #2770 (race-free apply / closes the self_purge "delete op-log while apply still writing it" window) **and** #2776 (its residue check must look for the AES `GroupKeyEntry` rows that #2776 made the purge delete).
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

**Namespace path:** `SubgroupCreated`/`SubgroupReparented` (`ops/namespace/*`, `NamespaceApplyCtx`, which has no sink today) run under the RootOp path that dedups *before* emit, so they are not racy in the #2770 sense. **Confirm the RootOp persist ordering during implementation**; only thread a sink there if the confirmation shows an emit-before-persist gap. Default expectation: no change needed on the namespace path.

**Follow-up (not in PR-1):** once both PR-1 and #2772 are merged, remove the now-redundant bounded wake-then-reread in `tee_subgroup_admit::handle_new_tee_member` (fold into PR-2 or a tiny standalone cleanup). Add a doc note on `GroupApplyCtx.pending_events`: future group handlers must `queue_event`, never `notify` directly, or they reintroduce the race.

**Tests:** a subscriber that reads the op-log on its triggering event observes the entry (race gone); a replayed op does not double-emit; existing governance-store + the three subscribers' tests stay green.

---

## PR-2 — #2726: subgroup purge-failure reconcile

**Problem.** A subgroup-only self-purge whose load-bearing delete partially fails leaves private key residue with no recovery: the `PurgeAction::Subgroup` path writes no marker, `purge_subgroup_for_self` returns no `Result`, and the startup `reconcile_sweep` (which only handles namespace-root markers) never retries it. The one-level-down mirror of the gap #2721 closed.

**Three steps:**

1. **`Result`-refactor `purge_subgroup_for_self`** (`self_purge.rs:759`): return `eyre::Result<()>`, `Err` **iff** the load-bearing `delete_group_local_rows` step (`self_purge.rs:800`) fails; best-effort context/tree-edge failures stay non-fatal (mirror the namespace two-class split). The caller marks-before / clears-on-`Ok` / leaves-on-`Err`.

2. **Marker storage A1** (confirmed over A2): a **new `calimero-store` key prefix + `PendingSubgroupPurge` key type + `PendingSubgroupPurgeRepository`**, each a 1:1 copy of `PendingSelfPurge`/`PendingSelfPurgeRepository`. Rationale: the marker key is `[prefix][32-byte id]` with a `()` value, so reusing the namespace marker type (A2) makes subgroup markers **byte-indistinguishable** from namespace markers — and disambiguating by re-resolving the namespace is impossible exactly in the failure case (the subgroup's tree edges are already deleted). A1 gives the sweep a disjoint, unambiguous enumerator. Pick the next free prefix byte (verify against the `*_PREFIX` list; `0x3F` appears free).
   - **Mark before purge** on the `PurgeAction::Subgroup` arm (`self_purge.rs:694`, symmetric to the namespace arm at `:707`); clear on `Ok`, leave on `Err`.

3. **Extend `reconcile_sweep`** with a subgroup pass over the new marker repo, applying a **3-state predicate** (checking the **subgroup `gid`**, not root — and it cannot reuse the namespace "identity-gone" signal since identity is namespace-scoped):
   - **healthy** — `MembershipRepository::role_of(gid, self_pk).is_some()` (direct role) → clear marker, don't purge.
   - **evicted-with-residue** — no role **and** residue present → re-run `purge_subgroup_for_self` (idempotent).
   - **already-purged** — no role **and** no residue → clear marker, don't purge.
   - **Residue = dual-family (the #2776 interaction):** `GroupKeyring::load_current_key_record(gid).is_some()` (AES `GroupKeyEntry`) **OR** `SigningKeysRepository::get_key(gid, self_pk).is_some()` (signing key). #2776 made `delete_group_local_rows` delete both, non-atomically with a first-error return, so a partial failure can leave "signing gone / AES present" — checking only signing keys would miss it.
   - Reuse `PurgeBranch::Subgroup` + extend `ReconcileOutcome` for metrics.

**Startup-only** sweep (periodic is a deferred follow-up). **Effort ~5–7 dev-days.**

**Tests:** mark-before-purge symmetry; `Result` drives clear/leave; reconcile re-runs a failed subgroup purge; the 3rd already-purged state clears without re-purging; residue detection covers both key families (incl. the signing-gone/AES-present case); no false-purge of a re-admitted subgroup member. Match #2725 coverage.

---

## PR-3 — #2771: eliminate the redundant transient admission (structural fix)

**Problem (real & structural, not cosmetic-only).** There is **no atomic create-Open path** — `CreateGroupRequest` carries no visibility, so every Open subgroup is born **Restricted** (default) then flipped via a separate `SubgroupVisibilitySet` op. In the window between create and flip, the `tee_subgroup_admit` create-time subscriber **reliably** admits the entitled TEE node into the (momentarily Restricted) subgroup — writing a direct `ReadOnlyTee` row **and delivering the per-subgroup key**. After the flip to Open, the node would have read the subgroup via inheritance + the namespace key anyway, so the admission + key delivery are **redundant**, on **every** Open-subgroup creation.

**Why not prune after the flip (rejected).** A silent local delete of the redundant row **breaks convergence**: `hash_group_state` hashes every `GroupMember` row, so a local-only delete diverges the subgroup state hash across peers → permanent governance desync. An op-based prune is disproportionate (a new replicated op in a security-sensitive path that would emit `TeeMemberRemoved` → trigger `self_purge` → strip the keys, the opposite of intent).

**Fix (structural): initial visibility on create.** Add an optional `visibility` to `CreateGroupRequest` (default Restricted, backward-compatible) and apply it atomically in the `GroupCreated` apply, so a subgroup intended to be Open is **born Open**. Then `is_open_chain_to_namespace` is already true when `SubgroupCreated` fires, the create-time subscriber correctly **skips** it (Open subgroups need no admission), and the redundant row + recurring wasted key delivery never happen. This eliminates the window rather than papering over it.

**Scope note:** touches `CreateGroupRequest` (`context/primitives`), the create-group handler, and the `GroupCreated` apply to set initial visibility; callers that want Open pass it. Lives in `create_group` (#2772-adjacent). No change to the visibility-flip path.

**Tests:** a subgroup created with `visibility: Open` is Open immediately (no flip); `tee_subgroup_admit` does NOT admit the TEE node into it (no redundant row, no key delivery); default (no visibility) stays Restricted (unchanged behavior); existing create-then-flip still works.

---

## Non-goals / out of scope

- Periodic reconcile sweep (PR-2 is startup-only; periodic is a later follow-up).
- The namespace-path emit refactor unless PR-1's ordering check shows a gap there.
- Any change to `GroupKeyDelivered` semantics.
- mdma / mero-tee — these are core-only changes; no contract-crate (`calimero-server-primitives`, `calimero-tee-attestation`) public-surface change, so no mero-tee rev bump.
